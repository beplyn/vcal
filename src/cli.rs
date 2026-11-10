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
    /// Print one event in full: times, recurrence, changed and cancelled
    /// occurrences, and the next few occurrences
    Show {
        /// Event id, as printed by `add` or `query`
        id: String,
        /// Print a JSON object instead
        #[arg(long)]
        json: bool,
        /// How many upcoming occurrences to list
        #[arg(long, value_name = "N", default_value_t = 5)]
        upcoming: usize,
    },
    /// Print the next event, for status bars
    Next(NextArgs),
    /// Send desktop notifications (notify-send) for reminders that are due;
    /// meant to run every minute from a timer
    Notify {
        /// Print the notifications instead of sending them, and don't mark them sent
        #[arg(long)]
        dry_run: bool,
    },
    /// Import events from an iCalendar (.ics) file, or - for stdin
    Import {
        #[arg(value_name = "FILE")]
        path: PathBuf,
    },
    /// Write events as an iCalendar (.ics) file, to stdout or FILE
    Export(ExportArgs),
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
    /// Only occurrences whose title, category, notes or location contain TEXT
    /// (ignoring case), like / in the TUI
    #[arg(long, short, value_name = "TEXT")]
    search: Option<String>,
    /// Only occurrences in this category (repeatable; ignoring case)
    #[arg(long, short, value_name = "NAME")]
    category: Vec<String>,
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
    /// Notify this long before the start, e.g. 10m or 1d (see `vcal notify`)
    #[arg(long, short = 'R', value_name = "DURATION")]
    reminder: Option<String>,
    #[arg(long, default_value = "confirmed")]
    status: Status,
}

#[derive(Args)]
#[command(
    after_help = "Only the options you give change. An empty string clears --category, \
--location, --description, --rrule or --reminder; with --occurrence it falls back to the series' value \
instead."
)]
struct EditArgs {
    /// Event id, as printed by `add` or `query`
    id: String,
    /// Change only this occurrence (the `occurrence` value from `query`)
    #[arg(long, value_name = "TS", conflicts_with_all = ["all_day", "timed", "rrule", "status", "reminder"])]
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
    /// New reminder lead time, e.g. 10m; "" removes the reminder
    #[arg(long, short = 'R', value_name = "DURATION")]
    reminder: Option<String>,
    #[arg(long)]
    status: Option<Status>,
}

#[derive(Args)]
#[command(
    after_help = "TEMPLATE fields: {title} {location} {category} {description} {id}; {start} and \
{end}, which take a strftime format as in {start:%a %H:%M}; {when} (\"14:00\", \"tomorrow 9:30\", \
\"Thu 9:30\"); {in} (\"in 25m\", or \"now\" once it has started). {{ and }} are literal braces. \
Prints nothing when there is no event."
)]
struct NextArgs {
    /// Output template [default: "{when} {title}"]
    #[arg(long, value_name = "TEMPLATE", conflicts_with = "json")]
    format: Option<String>,
    /// Print the event as a JSON object, with the fields of `query --format json`
    #[arg(long)]
    json: bool,
    /// How far ahead to look
    #[arg(long, value_name = "DURATION", default_value = "7d")]
    within: String,
    /// Also count an event that has started and not ended yet
    #[arg(long)]
    ongoing: bool,
    /// Include all-day events
    #[arg(long)]
    all_day: bool,
}

#[derive(Args)]
#[command(
    after_help = "With no filters, every event is written. Filters combine: an event must \
match all of the given kinds, and any one value of each."
)]
struct ExportArgs {
    /// Write here instead of stdout
    #[arg(value_name = "FILE")]
    path: Option<PathBuf>,
    /// Only events in this category (repeatable)
    #[arg(long, short, value_name = "NAME")]
    category: Vec<String>,
    /// Only this event (repeatable)
    #[arg(long, value_name = "ID")]
    id: Vec<String>,
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
        Some(Command::Show { id, json, upcoming }) => {
            show(&Db::open(&db_path)?, &id, json, upcoming, tz, &cfg.settings)
        }
        Some(Command::Next(args)) => next(&Db::open(&db_path)?, args, tz, &cfg.settings),
        Some(Command::Notify { dry_run }) => {
            notify(&mut Db::open(&db_path)?, dry_run, tz, &cfg.settings)
        }
        Some(Command::Import { path }) => import(&mut Db::open(&db_path)?, &path, tz),
        Some(Command::Export(args)) => export(&Db::open(&db_path)?, args),
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
    let occurrences: Vec<Occurrence> = db
        .occurrences(start, end, tz, args.include_cancelled)?
        .into_iter()
        .filter(|o| args.search.as_deref().is_none_or(|s| o.matches(s)))
        .filter(|o| args.category.is_empty() || o.in_category(&args.category))
        .collect();
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
    /// Minutes before the start; `null` for none
    reminder: Option<i64>,
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
            reminder: o.reminder,
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
    ev.reminder = args
        .reminder
        .as_deref()
        .map(time::parse_reminder)
        .transpose()?
        .flatten();
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
        || args.reminder.is_some()
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
    if let Some(r) = &args.reminder {
        ev.reminder = time::parse_reminder(r)?;
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

#[derive(Serialize)]
struct JsonException {
    occurrence: i64,
    kind: &'static str,
    title: Option<String>,
    description: Option<String>,
    category: Option<String>,
    location: Option<String>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
}

#[derive(Serialize)]
struct JsonEvent<'a> {
    id: &'a str,
    title: &'a str,
    description: &'a str,
    category: Option<&'a str>,
    location: Option<&'a str>,
    all_day: bool,
    start: String,
    end: String,
    start_ts: i64,
    end_ts: i64,
    tzid: Option<&'a str>,
    rrule: Option<&'a str>,
    status: Status,
    reminder: Option<i64>,
    created_ts: i64,
    updated_ts: i64,
    exceptions: Vec<JsonException>,
    upcoming: Vec<JsonOccurrence<'a>>,
}

/// An event's own span as a pseudo-occurrence, to share `bounds`.
fn as_occurrence(ev: &Event) -> Occurrence {
    Occurrence {
        event_id: ev.id.clone(),
        original_start: None,
        title: ev.title.clone(),
        description: ev.description.clone(),
        category: ev.category.clone(),
        location: ev.location.clone(),
        start: ev.start,
        end: ev.end,
        all_day: ev.all_day,
        status: ev.status,
        reminder: ev.reminder,
        modified: false,
    }
}

fn show(db: &Db, id: &str, json: bool, upcoming: usize, tz: Tz, settings: &Settings) -> Result<()> {
    let ev = db
        .get(id)?
        .with_context(|| format!("no event with id {id}"))?;
    let exceptions = db.exceptions(id)?;
    let now = time::now();
    // the next occurrences, looking up to a year ahead (all of them for a single event)
    let next: Vec<Occurrence> = if ev.is_recurring() {
        db.occurrences(now, now + 366 * DAY, tz, false)?
            .into_iter()
            .filter(|o| o.event_id == ev.id && bounds(o, tz).3 > now)
            .take(upcoming)
            .collect()
    } else {
        vec![]
    };
    let (start, end, start_ts, end_ts) = bounds(&as_occurrence(&ev), tz);

    if json {
        let row = JsonEvent {
            id: &ev.id,
            title: &ev.title,
            description: &ev.description,
            category: ev.category.as_deref(),
            location: ev.location.as_deref(),
            all_day: ev.all_day,
            start,
            end,
            start_ts,
            end_ts,
            tzid: ev.tzid.as_deref(),
            rrule: ev.rrule.as_deref(),
            status: ev.status,
            reminder: ev.reminder,
            created_ts: ev.created,
            updated_ts: ev.updated,
            exceptions: exceptions
                .iter()
                .map(|x| JsonException {
                    occurrence: x.original_start,
                    kind: x.kind.as_str(),
                    title: x.title.clone(),
                    description: x.description.clone(),
                    category: x.category.clone(),
                    location: x.location.clone(),
                    start_ts: x.start,
                    end_ts: x.end,
                })
                .collect(),
            upcoming: next.iter().map(|o| JsonOccurrence::new(o, tz)).collect(),
        };
        return emit(&(serde_json::to_string_pretty(&row)? + "\n"));
    }

    let stamp = |ts: i64| {
        let dt = time::in_tz(ts, tz);
        format!("{} {}", dt.format("%a %Y-%m-%d"), settings.time(dt))
    };
    // the span of something starting at `s` and ending at `e`, as people read it
    let span = |s: i64, e: i64| {
        if ev.all_day {
            let (a, b) = (
                time::floating_date(s),
                time::floating_date(e) - chrono::Duration::days(1),
            );
            if a == b {
                format!("{}, all day", a.format("%a %Y-%m-%d"))
            } else {
                format!(
                    "{} – {}, all day",
                    a.format("%a %Y-%m-%d"),
                    b.format("%a %Y-%m-%d")
                )
            }
        } else if time::local_date(s, tz) == time::local_date(e, tz) {
            format!("{}–{}", stamp(s), settings.time(time::in_tz(e, tz)))
        } else {
            format!("{} – {}", stamp(s), stamp(e))
        }
    };
    let mut out = String::new();
    let mut field = |name: &str, value: &str| {
        let _ = writeln!(out, "{name:<10}{}", one_line(value));
    };
    field("title", &ev.title);
    field("id", &ev.id);
    let mut when = span(ev.start, ev.end);
    if let Some(z) = ev.tzid.as_deref().filter(|z| *z != tz.name()) {
        let _ = write!(when, "  (series in {z})");
    }
    field("when", &when);
    if let Some(rule) = &ev.rrule {
        let ends = match recurrence::series_end(&ev)? {
            Some(last) => format!("last ends {}", stamp(last)),
            None => "no end".into(),
        };
        field("repeats", &format!("{rule}  ({ends})"));
    }
    if let Some(c) = &ev.category {
        field("category", c);
    }
    if let Some(l) = &ev.location {
        field("location", l);
    }
    field("status", ev.status.as_str());
    if let Some(m) = ev.reminder {
        field("reminder", &format!("{} before", time::format_minutes(m)));
    }
    field("created", &stamp(ev.created));
    field("updated", &stamp(ev.updated));
    for x in &exceptions {
        let at = if ev.all_day {
            time::floating_date(x.original_start)
                .format("%a %Y-%m-%d")
                .to_string()
        } else {
            stamp(x.original_start)
        };
        match x.kind {
            ExceptionKind::Cancelled => {
                field("cancelled", &format!("{at}  [{}]", x.original_start))
            }
            ExceptionKind::Modified => {
                let mut what = vec![];
                if let (Some(s), Some(e)) = (x.start, x.end) {
                    what.push(format!("→ {}", span(s, e)));
                }
                if let Some(t) = &x.title {
                    what.push(format!("\"{t}\""));
                }
                for (name, v) in [("category", &x.category), ("location", &x.location)] {
                    if let Some(v) = v {
                        what.push(format!("{name} {v}"));
                    }
                }
                if x.description.is_some() {
                    what.push("own notes".into());
                }
                field(
                    "changed",
                    &format!("{at} {}  [{}]", what.join(", "), x.original_start),
                );
            }
        }
    }
    for (i, o) in next.iter().enumerate() {
        let label = if i == 0 { "upcoming" } else { "" };
        let mut line = span(o.start, o.end);
        if o.modified {
            line.push_str("  (changed)");
        }
        let _ = write!(line, "  [{}]", o.original_start.unwrap_or(o.start));
        field(label, &line);
    }
    if !ev.description.is_empty() {
        out.push_str("notes\n");
        for l in ev.description.lines() {
            let _ = writeln!(out, "  {l}");
        }
    }
    emit(&out)
}

/// The occurrence's real start instant (local midnight for all-day events).
fn start_instant(o: &Occurrence, tz: Tz) -> i64 {
    bounds(o, tz).2
}

/// "14:00", "tomorrow 9:30", "Thu 9:30", or for all-day events "today",
/// "tomorrow", "Thu".
fn when_label(o: &Occurrence, tz: Tz, now: i64, settings: &Settings) -> String {
    let (day, time) = if o.all_day {
        (time::floating_date(o.start), None)
    } else {
        let dt = time::in_tz(o.start, tz);
        (dt.date_naive(), Some(settings.time(dt)))
    };
    let today = time::local_date(now, tz);
    let day = match (day - today).num_days() {
        0 => o.all_day.then(|| "today".to_string()),
        1 => Some("tomorrow".into()),
        n if n < 0 => Some(day.format("%Y-%m-%d").to_string()),
        _ => Some(day.format("%a").to_string()),
    };
    [day, time]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fills a `next --format` template.
fn render_template(
    template: &str,
    o: &Occurrence,
    tz: Tz,
    now: i64,
    settings: &Settings,
) -> Result<String> {
    use chrono::format::{Item, StrftimeItems};
    let (start_ts, end_ts) = {
        let (.., s, e) = bounds(o, tz);
        (s, e)
    };
    let stamp = |ts: i64, spec: Option<&str>| -> Result<String> {
        let dt = time::in_tz(ts, tz);
        Ok(match spec {
            Some(spec) => {
                ensure!(
                    !StrftimeItems::new(spec).any(|i| matches!(i, Item::Error)),
                    "bad time format {spec:?} in --format"
                );
                dt.format(spec).to_string()
            }
            None if o.all_day => dt.format("%Y-%m-%d").to_string(),
            None => settings.time(dt),
        })
    };
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        ensure!(
            tail.starts_with('{'),
            "unmatched }} in --format (write }}}} for a literal one)"
        );
        let close = tail
            .find('}')
            .context("unclosed { in --format (write {{ for a literal one)")?;
        let field = &tail[1..close];
        let (name, spec) = match field.split_once(':') {
            Some((n, s)) => (n, Some(s)),
            None => (field, None),
        };
        let value = match (name, spec) {
            ("title", None) => one_line(&o.title),
            ("location", None) => one_line(o.location.as_deref().unwrap_or("")),
            ("category", None) => one_line(o.category.as_deref().unwrap_or("")),
            ("description", None) => one_line(&o.description),
            ("id", None) => o.event_id.clone(),
            ("when", None) => when_label(o, tz, now, settings),
            ("in", None) if start_ts <= now => "now".into(),
            // round up, so "in 1m" until it starts
            ("in", None) => format!("in {}", time::format_minutes((start_ts - now + 59) / 60)),
            ("start", spec) => stamp(start_ts, spec)?,
            // an all-day end is exclusive; show the last day
            ("end", spec) if o.all_day => stamp(end_ts - DAY, spec)?,
            ("end", spec) => stamp(end_ts, spec)?,
            _ => bail!(
                "unknown field {{{field}}} in --format (title, location, category, \
                 description, id, when, in, start, end)"
            ),
        };
        out.push_str(&value);
        rest = &tail[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn next(db: &Db, args: NextArgs, tz: Tz, settings: &Settings) -> Result<()> {
    let now = time::now();
    let within = time::parse_duration(&args.within)?;
    ensure!(within > 0, "--within must be positive");
    let found = db
        .occurrences(now, now + within, tz, false)?
        .into_iter()
        .filter(|o| args.all_day || !o.all_day)
        .filter(|o| {
            let (.., start, end) = bounds(o, tz);
            start >= now || (args.ongoing && end > now)
        })
        .min_by_key(|o| start_instant(o, tz));
    let Some(o) = found else {
        return Ok(());
    };
    emit(&if args.json {
        serde_json::to_string(&JsonOccurrence::new(&o, tz))? + "\n"
    } else {
        let template = args.format.as_deref().unwrap_or("{when} {title}");
        render_template(template, &o, tz, now, settings)? + "\n"
    })
}

/// How long after an occurrence starts its reminder may still go out (say
/// the machine was asleep when it fell due).
const NOTIFY_LATE: i64 = 15 * 60;

fn notify(db: &mut Db, dry_run: bool, tz: Tz, settings: &Settings) -> Result<()> {
    let now = time::now();
    let lead = db.max_reminder()? * 60;
    let mut sent = Vec::new();
    let mut failure = None;
    for o in db.occurrences(now - NOTIFY_LATE, now + lead + 1, tz, false)? {
        let Some(minutes) = o.reminder else { continue };
        let start = start_instant(&o, tz);
        if now < start - minutes * 60 || now >= start + NOTIFY_LATE {
            continue;
        }
        if db.was_notified(&o.event_id, start)? {
            continue;
        }
        let mut body = when_label(&o, tz, now, settings);
        if !o.all_day {
            let _ = write!(body, "–{}", settings.time(time::in_tz(o.end, tz)));
        }
        if let Some(l) = &o.location {
            let _ = write!(body, " · {l}");
        }
        if dry_run {
            emit(&format!("{}\t{body}\n", one_line(&o.title)))?;
            continue;
        }
        let status = std::process::Command::new("notify-send")
            .args(["--app-name=vcal", "--icon=x-office-calendar", "--"])
            .arg(&o.title)
            .arg(&body)
            .status();
        match status {
            Ok(s) if s.success() => sent.push((o.event_id.clone(), start)),
            Ok(s) => failure = Some(anyhow::anyhow!("notify-send failed ({s})")),
            Err(e) => failure = Some(anyhow::Error::new(e).context("can't run notify-send")),
        }
    }
    if !sent.is_empty() {
        db.mark_notified(&sent, now, now - 2 * DAY)?;
    }
    failure.map_or(Ok(()), Err)
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

fn export(db: &Db, args: ExportArgs) -> Result<()> {
    let events: Vec<Event> = db
        .events()?
        .into_iter()
        .filter(|e| args.id.is_empty() || args.id.contains(&e.id))
        .filter(|e| {
            args.category.is_empty()
                || e.category
                    .as_deref()
                    .is_some_and(|c| args.category.iter().any(|w| w.eq_ignore_ascii_case(c)))
        })
        .collect();
    for id in &args.id {
        ensure!(db.get(id)?.is_some(), "no event with id {id}");
    }
    let n = events.len();
    let text = ics::write::calendar(&db.with_exceptions(events)?, time::now());
    match &args.path {
        Some(p) if p != Path::new("-") => {
            std::fs::write(p, text).with_context(|| format!("can't write {}", p.display()))?;
            eprintln!("exported {n} events to {}", p.display());
            Ok(())
        }
        _ => emit(&text),
    }
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
