//! Command-line interface: the TUI entry point plus headless subcommands for
//! scripts and the desktop shell.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::db::{self, Db};
use crate::ics;
use crate::model::{Event, Occurrence, Status};
use crate::settings::{Settings, View};
use crate::time::{self, DAY, When};

#[derive(Parser)]
#[command(
    name = "vcal",
    version,
    about = "A Vim-keybound terminal calendar backed by SQLite"
)]
#[command(
    after_help = "Times (<WHEN>) accept: now, today, tomorrow, yesterday (optionally \
followed by HH:MM), +2h / -3d / +1w relative to now, epoch seconds, RFC 3339, YYYY-MM-DD, or \
\"YYYY-MM-DD HH:MM\" in local time."
)]
pub struct Cli {
    /// Database file [default: $XDG_DATA_HOME/calendar/events.db]
    #[arg(long, global = true, env = "VCAL_DB", value_name = "PATH")]
    db: Option<PathBuf>,

    /// Open the TUI on this date
    #[arg(long, value_name = "WHEN")]
    date: Option<String>,

    /// Open the TUI in this view
    #[arg(long, value_enum)]
    view: Option<View>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the occurrences in a time window
    Query(QueryArgs),
    /// Create an event and print its id
    Add(AddArgs),
    /// Delete an event, or one occurrence of a recurring event
    Rm(RmArgs),
    /// Import events from an iCalendar (.ics) file, or - for stdin
    Import {
        #[arg(value_name = "FILE")]
        path: PathBuf,
    },
    /// Write the man page and shell completions into DIR (used for packaging)
    #[command(hide = true)]
    GenAssets {
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },
}

#[derive(Args)]
struct QueryArgs {
    /// Window start
    #[arg(long, value_name = "WHEN", default_value = "today")]
    from: String,
    /// Window end, exclusive; a bare date includes that whole day
    /// [default: one day after --from]
    #[arg(long, value_name = "WHEN")]
    to: Option<String>,
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Include cancelled events
    #[arg(long)]
    include_cancelled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
    Tsv,
}

#[derive(Args)]
struct AddArgs {
    #[arg(long, short)]
    title: String,
    /// Start time, or the first day of an all-day event
    #[arg(long, short, value_name = "WHEN")]
    start: String,
    /// End time; for all-day events, the last day (inclusive)
    #[arg(long, short, value_name = "WHEN", conflicts_with = "duration")]
    end: Option<String>,
    /// Length such as 30m, 1h30m or 2d [default: 1h, or 1d for all-day events]
    #[arg(long, short = 'D', value_name = "DURATION")]
    duration: Option<String>,
    /// Make it an all-day event
    #[arg(long, short)]
    all_day: bool,
    #[arg(long, short)]
    category: Option<String>,
    #[arg(long, short = 'n', value_name = "TEXT")]
    description: Option<String>,
    #[arg(long, short)]
    location: Option<String>,
    /// RFC 5545 recurrence rule, e.g. "FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20270101"
    #[arg(long, short, value_name = "RULE")]
    rrule: Option<String>,
    #[arg(long, default_value = "confirmed")]
    status: Status,
}

#[derive(Args)]
struct RmArgs {
    /// Event id, as printed by `add` or `query`
    id: String,
    /// Only cancel this occurrence (the `occurrence` value from `query`)
    #[arg(long, value_name = "TS")]
    occurrence: Option<i64>,
}

pub fn run() -> Result<()> {
    run_with(Cli::parse())
}

fn run_with(cli: Cli) -> Result<()> {
    let tz = time::local_tz();
    let db_path = cli.db.clone().unwrap_or_else(db::default_path);
    match cli.command {
        None => {
            let date = cli
                .date
                .map(|d| time::parse_when(&d, tz, time::now()).map(|w| w.date(tz)))
                .transpose()?;
            crate::tui::run(Db::open(&db_path)?, Settings::default(), date, cli.view)
        }
        Some(Command::Query(args)) => query(&Db::open(&db_path)?, args, tz),
        Some(Command::Add(args)) => add(&mut Db::open(&db_path)?, args, tz),
        Some(Command::Rm(args)) => rm(&mut Db::open(&db_path)?, args),
        Some(Command::Import { path }) => import(&mut Db::open(&db_path)?, &path, tz),
        Some(Command::GenAssets { dir }) => gen_assets(&dir),
    }
}

/// Writes to stdout, treating a closed pipe (`vcal query | head`) as success.
fn emit(text: &str) -> Result<()> {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

fn query(db: &Db, args: QueryArgs, tz: Tz) -> Result<()> {
    let now = time::now();
    let from = time::parse_when(&args.from, tz, now)?;
    let start = from.instant(tz);
    let end = match args
        .to
        .as_deref()
        .map(|s| time::parse_when(s, tz, now))
        .transpose()?
    {
        Some(When::Date(d)) => time::local_midnight(d.succ_opt().unwrap_or(d), tz),
        Some(When::Instant(ts)) => ts,
        None => match from {
            When::Date(d) => time::local_midnight(d.succ_opt().unwrap_or(d), tz),
            When::Instant(ts) => ts + DAY,
        },
    };
    ensure!(end >= start, "--to is before --from");
    let occurrences = db.occurrences(start, end, tz, args.include_cancelled)?;
    emit(&match args.format {
        Format::Json => {
            let rows: Vec<_> = occurrences
                .iter()
                .map(|o| JsonOccurrence::new(o, tz))
                .collect();
            serde_json::to_string_pretty(&rows)? + "\n"
        }
        Format::Tsv => tsv(&occurrences, tz),
        Format::Text => text(&occurrences, tz),
    })
}

/// One occurrence as `query --format json` prints it. All-day events give
/// dates (`end` exclusive) and local-midnight timestamps; `occurrence` is
/// the opaque handle `rm --occurrence` takes.
#[derive(Serialize)]
struct JsonOccurrence<'a> {
    id: &'a str,
    occurrence: Option<i64>,
    title: &'a str,
    description: &'a str,
    category: Option<&'a str>,
    location: Option<&'a str>,
    all_day: bool,
    start: String,
    end: String,
    start_ts: i64,
    end_ts: i64,
    status: Status,
    recurring: bool,
    modified: bool,
}

impl<'a> JsonOccurrence<'a> {
    fn new(o: &'a Occurrence, tz: Tz) -> Self {
        let (start, end, start_ts, end_ts) = bounds(o, tz);
        JsonOccurrence {
            id: &o.event_id,
            occurrence: o.original_start,
            title: &o.title,
            description: &o.description,
            category: o.category.as_deref(),
            location: o.location.as_deref(),
            all_day: o.all_day,
            start,
            end,
            start_ts,
            end_ts,
            status: o.status,
            recurring: o.recurring(),
            modified: o.modified,
        }
    }
}

/// Display start/end strings and real instants for an occurrence.
fn bounds(o: &Occurrence, tz: Tz) -> (String, String, i64, i64) {
    if o.all_day {
        let (s, e) = (time::floating_date(o.start), time::floating_date(o.end));
        (
            s.to_string(),
            e.to_string(),
            time::local_midnight(s, tz),
            time::local_midnight(e, tz),
        )
    } else {
        let fmt = |ts| {
            time::in_tz(ts, tz)
                .format("%Y-%m-%dT%H:%M:%S%:z")
                .to_string()
        };
        (fmt(o.start), fmt(o.end), o.start, o.end)
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn tsv(occurrences: &[Occurrence], tz: Tz) -> String {
    let mut out = String::from("id\toccurrence\tstart\tend\tall_day\tstatus\tcategory\ttitle\n");
    for o in occurrences {
        let (start, end, ..) = bounds(o, tz);
        let occ = o.original_start.map(|t| t.to_string()).unwrap_or_default();
        let _ = writeln!(
            out,
            "{}\t{occ}\t{start}\t{end}\t{}\t{}\t{}\t{}",
            o.event_id,
            o.all_day,
            o.status,
            one_line(o.category.as_deref().unwrap_or("")),
            one_line(&o.title),
        );
    }
    out
}

fn text(occurrences: &[Occurrence], tz: Tz) -> String {
    let mut out = String::new();
    let mut last_day = None;
    for o in occurrences {
        let day = if o.all_day {
            time::floating_date(o.start)
        } else {
            time::local_date(o.start, tz)
        };
        let label = if last_day == Some(day) {
            " ".repeat(14)
        } else {
            day.format("%a %Y-%m-%d").to_string()
        };
        last_day = Some(day);
        let when = if o.all_day {
            let days = (o.end - o.start) / DAY;
            if days > 1 {
                format!("all day ×{days}")
            } else {
                "all day".into()
            }
        } else {
            let hm = |ts| time::in_tz(ts, tz).format("%H:%M").to_string();
            format!("{}–{}", hm(o.start), hm(o.end))
        };
        let mut line = format!("{label}  {when:<11}  {}", one_line(&o.title));
        if let Some(c) = &o.category {
            let _ = write!(line, "  [{}]", one_line(c));
        }
        if o.recurring() {
            line.push_str("  ↻");
        }
        if o.status != Status::Confirmed {
            let _ = write!(line, "  ({})", o.status);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn add(db: &mut Db, args: AddArgs, tz: Tz) -> Result<()> {
    let now = time::now();
    let start = time::parse_when(&args.start, tz, now)?;
    let end = args
        .end
        .as_deref()
        .map(|s| time::parse_when(s, tz, now))
        .transpose()?;
    let duration = args
        .duration
        .as_deref()
        .map(time::parse_duration)
        .transpose()?;

    let (start_ts, end_ts) = if args.all_day {
        let first = start.date(tz);
        let after_last = match (end, duration) {
            (Some(e), _) => e.date(tz).succ_opt().context("end date out of range")?,
            (None, Some(d)) => {
                ensure!(
                    d > 0 && d % DAY == 0,
                    "all-day durations are whole days (e.g. 2d)"
                );
                first + chrono::Duration::days(d / DAY)
            }
            (None, None) => first.succ_opt().context("date out of range")?,
        };
        ensure!(after_last > first, "the last day is before the first");
        (time::floating(first), time::floating(after_last))
    } else {
        let When::Instant(s) = start else {
            bail!(
                "--start {:?} is a date; add a time, or pass --all-day",
                args.start
            );
        };
        let e = match (end, duration) {
            (Some(e), _) => e.instant(tz),
            (None, Some(d)) => s + d,
            (None, None) => s + 3600,
        };
        (s, e)
    };

    let mut ev = Event::new(args.title, start_ts, end_ts);
    ev.all_day = args.all_day;
    ev.category = args.category;
    ev.description = args.description.unwrap_or_default();
    ev.location = args.location;
    ev.rrule = args.rrule;
    ev.status = args.status;
    if !ev.all_day {
        ev.tzid = Some(tz.name().to_string());
    }
    let saved = db.insert(ev)?;
    emit(&format!("{}\n", saved.id))
}

fn rm(db: &mut Db, args: RmArgs) -> Result<()> {
    match args.occurrence {
        Some(ts) => db.cancel_occurrence(&args.id, ts),
        None => {
            ensure!(db.delete(&args.id)?, "no event with id {}", args.id);
            Ok(())
        }
    }
}

fn import(db: &mut Db, path: &Path, tz: Tz) -> Result<()> {
    let mut input = String::new();
    if path == Path::new("-") {
        io::stdin().read_to_string(&mut input)?;
    } else {
        input = std::fs::read_to_string(path)
            .with_context(|| format!("can't read {}", path.display()))?;
    }
    let parsed = ics::parse(&input, tz)?;
    for w in &parsed.warnings {
        eprintln!("vcal: {w}");
    }
    let (events, exceptions) = db.import(parsed.events, parsed.exceptions)?;
    eprintln!("imported {events} events, {exceptions} exceptions");
    Ok(())
}

fn gen_assets(dir: &Path) -> Result<()> {
    use clap_complete::Shell;
    std::fs::create_dir_all(dir)?;
    let mut cmd = Cli::command();
    let mut page = Vec::new();
    clap_mangen::Man::new(cmd.clone()).render(&mut page)?;
    std::fs::write(dir.join("vcal.1"), page)?;
    for (shell, file) in [
        (Shell::Bash, "vcal.bash"),
        (Shell::Fish, "vcal.fish"),
        (Shell::Zsh, "_vcal"),
    ] {
        let mut out = Vec::new();
        clap_complete::generate(shell, &mut cmd, "vcal", &mut out);
        std::fs::write(dir.join(file), out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
