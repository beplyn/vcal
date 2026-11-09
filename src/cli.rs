//! Command-line interface: the TUI entry point plus headless subcommands for
//! scripts and the desktop shell.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::config::{self, Config};
use crate::db::{self, Db, recurrence};
use crate::ics;
use crate::model::{Event, Exception, ExceptionKind, Occurrence, Status};
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
    /// Database file [default: database_path from the config, else
    /// $XDG_DATA_HOME/calendar/events.db]
    #[arg(long, global = true, env = "VCAL_DB", value_name = "PATH")]
    db: Option<PathBuf>,

    /// Config file [default: $XDG_CONFIG_HOME/calendar/config.toml]
    #[arg(long, global = true, env = "VCAL_CONFIG", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Open the TUI on this date
    #[arg(long, value_name = "WHEN")]
    date: Option<String>,

    /// Open the TUI in this view [default: views.start from the config]
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
    /// Change an event, or one occurrence of a recurring event, and print its id
    Edit(EditArgs),
    /// Delete an event, or one occurrence of a recurring event
    Rm(RmArgs),
    /// Import events from an iCalendar (.ics) file, or - for stdin
    Import {
        #[arg(value_name = "FILE")]
        path: PathBuf,
    },
    /// Inspect the configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Write the man page and shell completions into DIR (used for packaging)
    #[command(hide = true)]
    GenAssets {
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the config file's path
    Path,
    /// Print the documented default config
    Default,
    /// Validate the config file without starting anything
    Check,
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
    /// Length such as 30m, 1h30m or 2d [default: general.default_duration, or
    /// 1d for all-day events]
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
#[command(
    after_help = "Only the options you give change. An empty string clears --category, \
--location, --description or --rrule; with --occurrence it falls back to the series' value \
instead."
)]
struct EditArgs {
    /// Event id, as printed by `add` or `query`
    id: String,
    /// Change only this occurrence (the `occurrence` value from `query`)
    #[arg(long, value_name = "TS", conflicts_with_all = ["all_day", "timed", "rrule", "status"])]
    occurrence: Option<i64>,
    #[arg(long, short)]
    title: Option<String>,
    /// New start; on its own, moves the event and keeps its length
    #[arg(long, short, value_name = "WHEN")]
    start: Option<String>,
    /// New end; for all-day events, the last day (inclusive)
    #[arg(long, short, value_name = "WHEN", conflicts_with = "duration")]
    end: Option<String>,
    /// New length, e.g. 45m or 2d
    #[arg(long, short = 'D', value_name = "DURATION")]
    duration: Option<String>,
    /// Make it an all-day event
    #[arg(long, short, conflicts_with = "timed")]
    all_day: bool,
    /// Make it a timed event (needs --start with a time)
    #[arg(long)]
    timed: bool,
    #[arg(long, short)]
    category: Option<String>,
    #[arg(long, short = 'n', value_name = "TEXT")]
    description: Option<String>,
    #[arg(long, short)]
    location: Option<String>,
    /// New recurrence rule; "" makes it a single event
    #[arg(long, short, value_name = "RULE")]
    rrule: Option<String>,
    #[arg(long)]
    status: Option<Status>,
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
    let config_path = cli.config.clone().unwrap_or_else(config::default_path);
    let command = match cli.command {
        Some(Command::GenAssets { dir }) => return gen_assets(&dir),
        Some(Command::Config { action }) => return config_command(action, &config_path),
        other => other,
    };
    let (cfg, note) = config::load_or_create(&config_path)?;
    let db_path = cli
        .db
        .clone()
        .or_else(|| cfg.database_path.clone())
        .unwrap_or_else(db::default_path);
    if command.is_some()
        && let Some(n) = &note
    {
        eprintln!("vcal: {n}");
    }
    match command {
        None => {
            let date = cli
                .date
                .map(|d| time::parse_when(&d, tz, time::now()).map(|w| w.date(tz)))
                .transpose()?;
            let launch = crate::tui::Launch {
                date,
                view: cli.view,
                config_path: Some(config_path),
                note,
            };
            crate::tui::run(Db::open(&db_path)?, cfg, launch)
        }
        Some(Command::Query(args)) => query(&Db::open(&db_path)?, args, tz, &cfg.settings),
        Some(Command::Add(args)) => add(&mut Db::open(&db_path)?, args, tz, &cfg),
        Some(Command::Edit(args)) => edit(&mut Db::open(&db_path)?, args, tz, &cfg),
        Some(Command::Rm(args)) => rm(&mut Db::open(&db_path)?, args),
        Some(Command::Import { path }) => import(&mut Db::open(&db_path)?, &path, tz),
        Some(Command::GenAssets { .. } | Command::Config { .. }) => unreachable!(),
    }
}

/// Writes to stdout, treating a closed pipe (`vcal query | head`) as success.
fn emit(text: &str) -> Result<()> {
    match io::stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

fn config_command(action: ConfigCommand, path: &Path) -> Result<()> {
    match action {
        ConfigCommand::Path => emit(&format!("{}\n", path.display())),
        ConfigCommand::Default => emit(config::DEFAULT_TOML),
        ConfigCommand::Check if !path.exists() => emit(&format!(
            "no config at {}; the defaults apply\n",
            path.display()
        )),
        ConfigCommand::Check => {
            config::load(path)?;
            emit(&format!("ok: {}\n", path.display()))
        }
    }
}

fn query(db: &Db, args: QueryArgs, tz: Tz, settings: &Settings) -> Result<()> {
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
        Format::Text => text(&occurrences, tz, settings),
    })
}

/// One occurrence as `query --format json` prints it. All-day events give
/// dates (`end` exclusive) and local-midnight timestamps; `occurrence` is
/// the opaque handle `edit --occurrence` and `rm --occurrence` take.
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

fn text(occurrences: &[Occurrence], tz: Tz, settings: &Settings) -> String {
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
            let hm = |ts| settings.time(time::in_tz(ts, tz));
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

/// `--start` / `--end` / `--duration` as given on the command line.
struct Times<'a> {
    start: Option<&'a str>,
    end: Option<&'a str>,
    duration: Option<&'a str>,
}

impl Times<'_> {
    fn any(&self) -> bool {
        self.start.is_some() || self.end.is_some() || self.duration.is_some()
    }
}

/// Stored `(start, end)` for the given times. `current` is the existing
/// `(start, end, all_day)` when editing: whatever isn't given comes from it,
/// and a lone `--start` keeps the length. All-day ends are inclusive days.
fn resolve_span(
    t: &Times,
    all_day: bool,
    current: Option<(i64, i64, bool)>,
    tz: Tz,
    default_duration: i64,
) -> Result<(i64, i64)> {
    let now = time::now();
    let start = t.start.map(|s| time::parse_when(s, tz, now)).transpose()?;
    let end = t.end.map(|s| time::parse_when(s, tz, now)).transpose()?;
    let duration = t.duration.map(time::parse_duration).transpose()?;

    if all_day {
        let local = |ts: i64, all_day: bool| {
            if all_day {
                time::floating_date(ts)
            } else {
                time::local_date(ts, tz)
            }
        };
        let first = match (start, current) {
            (Some(s), _) => s.date(tz),
            (None, Some((s, _, was_all_day))) => local(s, was_all_day),
            (None, None) => bail!("--start is required"),
        };
        let days = match (end, duration, current) {
            (Some(e), _, _) => (e.date(tz) - first).num_days() + 1,
            (None, Some(d), _) => {
                ensure!(
                    d > 0 && d % DAY == 0,
                    "all-day durations are whole days (e.g. 2d)"
                );
                d / DAY
            }
            (None, None, Some((s, e, true))) => (e - s) / DAY,
            (None, None, Some((s, e, false))) => {
                (time::local_date((e - 1).max(s), tz) - time::local_date(s, tz)).num_days() + 1
            }
            (None, None, None) => 1,
        };
        ensure!(days >= 1, "the last day is before the first");
        let start = time::floating(first);
        Ok((start, start + days * DAY))
    } else {
        let s = match (start, current) {
            (Some(When::Instant(s)), _) => s,
            (Some(When::Date(_)), _) => {
                bail!("--start is a date; add a time, or make it an all-day event")
            }
            (None, Some((s, _, false))) => s,
            (None, Some((_, _, true))) => bail!("a timed event needs --start with a time"),
            (None, None) => bail!("--start is required"),
        };
        let e = match (end, duration, current) {
            (Some(e), _, _) => e.instant(tz),
            (None, Some(d), _) => s + d,
            (None, None, Some((cs, ce, false))) => s + (ce - cs),
            (None, None, _) => s + default_duration,
        };
        ensure!(e >= s, "the end is before the start");
        Ok((s, e))
    }
}

fn add(db: &mut Db, args: AddArgs, tz: Tz, cfg: &Config) -> Result<()> {
    let times = Times {
        start: Some(&args.start),
        end: args.end.as_deref(),
        duration: args.duration.as_deref(),
    };
    let (start, end) = resolve_span(
        &times,
        args.all_day,
        None,
        tz,
        cfg.settings.default_duration,
    )?;
    let mut ev = Event::new(args.title, start, end);
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

fn edit(db: &mut Db, args: EditArgs, tz: Tz, cfg: &Config) -> Result<()> {
    let ev = db
        .get(&args.id)?
        .with_context(|| format!("no event with id {}", args.id))?;
    let times = Times {
        start: args.start.as_deref(),
        end: args.end.as_deref(),
        duration: args.duration.as_deref(),
    };
    let anything = times.any()
        || args.title.is_some()
        || args.category.is_some()
        || args.description.is_some()
        || args.location.is_some()
        || args.rrule.is_some()
        || args.status.is_some()
        || args.all_day
        || args.timed;
    ensure!(
        anything,
        "nothing to change; give --title, --start, --end, --category, ... (see vcal edit --help)"
    );
    let id = ev.id.clone();
    let default_duration = cfg.settings.default_duration;
    match args.occurrence {
        Some(at) => edit_occurrence(db, ev, at, &args, &times, tz, default_duration)?,
        None => edit_series(db, ev, &args, &times, tz, default_duration)?,
    }
    emit(&format!("{id}\n"))
}

fn edit_series(
    db: &mut Db,
    mut ev: Event,
    args: &EditArgs,
    times: &Times,
    tz: Tz,
    default_duration: i64,
) -> Result<()> {
    let all_day = if args.all_day {
        true
    } else if args.timed {
        false
    } else {
        ev.all_day
    };
    let old_start = ev.start;
    if times.any() || all_day != ev.all_day {
        let current = Some((ev.start, ev.end, ev.all_day));
        (ev.start, ev.end) = resolve_span(times, all_day, current, tz, default_duration)?;
        if !all_day && ev.all_day {
            ev.tzid = Some(tz.name().to_string());
        }
        ev.all_day = all_day;
    }
    if let Some(t) = &args.title {
        ev.title = t.clone();
    }
    if let Some(d) = &args.description {
        ev.description = d.clone();
    }
    // empty strings become None when the event is saved
    if let Some(c) = &args.category {
        ev.category = Some(c.clone());
    }
    if let Some(l) = &args.location {
        ev.location = Some(l.clone());
    }
    if let Some(r) = &args.rrule {
        ev.rrule = Some(r.clone());
    }
    if let Some(s) = args.status {
        ev.status = s;
    }
    let had_exceptions = ev.is_recurring() && !db.exceptions(&ev.id)?.is_empty();
    let saved = db.update(ev)?;
    if had_exceptions && saved.start != old_start {
        eprintln!(
            "vcal: this series has per-occurrence changes; they stay attached to the old \
             occurrence times"
        );
    }
    Ok(())
}

fn edit_occurrence(
    db: &mut Db,
    ev: Event,
    at: i64,
    args: &EditArgs,
    times: &Times,
    tz: Tz,
    default_duration: i64,
) -> Result<()> {
    ensure!(
        ev.is_recurring(),
        "event {} doesn't repeat; edit it without --occurrence",
        ev.id
    );
    let existing = db
        .exceptions(&ev.id)?
        .into_iter()
        .find(|x| x.original_start == at);
    match &existing {
        Some(x) if x.kind == ExceptionKind::Cancelled => {
            bail!("that occurrence of {} is cancelled", ev.id)
        }
        Some(_) => {}
        None => ensure!(
            recurrence::starts_between(&ev, at, at + 1)?.contains(&at),
            "event {} has no occurrence at {at} (use the `occurrence` value from `vcal query`)",
            ev.id
        ),
    }
    let mut x = existing.unwrap_or_else(|| Exception::modified(&ev.id, at));
    if times.any() {
        let current = (
            x.start.unwrap_or(at),
            x.end.unwrap_or(at + ev.duration()),
            ev.all_day,
        );
        let (s, e) = resolve_span(times, ev.all_day, Some(current), tz, default_duration)?;
        (x.start, x.end) = (Some(s), Some(e));
    }
    let nonempty = |s: &String| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(t) = &args.title {
        x.title = nonempty(t);
    }
    if let Some(d) = &args.description {
        x.description = nonempty(d);
    }
    if let Some(c) = &args.category {
        x.category = nonempty(c);
    }
    if let Some(l) = &args.location {
        x.location = nonempty(l);
    }
    db.set_exception(x)
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
