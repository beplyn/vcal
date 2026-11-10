//! `vcal doctor` (checks that the database, its events and the config are
//! sound) and `vcal backup` (a consistent copy of the database).

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::config;
use crate::db::{self, Db, recurrence};
use crate::model::ExceptionKind;
use crate::time::{self, DAY};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.level {
            Level::Ok => "ok  ",
            Level::Warn => "warn",
            Level::Fail => "FAIL",
        };
        write!(f, "{tag}  {}", self.text)
    }
}

#[derive(Default)]
struct Report(Vec<Finding>);

impl Report {
    fn push(&mut self, level: Level, text: impl Into<String>) {
        self.0.push(Finding {
            level,
            text: text.into(),
        });
    }
}

/// Every check, in the order they're shown. `config_path` is checked when
/// the file exists.
pub fn doctor(db: &Db, config_path: &Path) -> Result<Vec<Finding>> {
    let mut r = Report::default();
    let conn = db.conn();

    match db.path() {
        Some(p) => {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            r.push(
                Level::Ok,
                format!("database {} ({} KiB)", p.display(), size.div_ceil(1024)),
            );
            let sentinel = db::sentinel_path(p);
            match std::fs::OpenOptions::new().append(true).open(&sentinel) {
                Ok(_) => r.push(
                    Level::Ok,
                    format!("change file {} is writable", sentinel.display()),
                ),
                Err(e) => r.push(
                    Level::Warn,
                    format!(
                        "can't write {} ({e}); widgets won't see changes",
                        sentinel.display()
                    ),
                ),
            }
        }
        None => r.push(Level::Ok, "database in memory"),
    }

    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == db::schema_version() {
        r.push(Level::Ok, format!("schema v{version}"));
    } else {
        r.push(
            Level::Fail,
            format!("schema v{version}, expected v{}", db::schema_version()),
        );
    }
    if db.path().is_some() {
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        if mode.eq_ignore_ascii_case("wal") {
            r.push(Level::Ok, "WAL journal");
        } else {
            r.push(Level::Warn, format!("journal mode is {mode}, not WAL"));
        }
    }

    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let problems: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if problems == ["ok"] {
        r.push(Level::Ok, "SQLite integrity check");
    } else {
        for p in problems.iter().take(20) {
            r.push(Level::Fail, format!("integrity: {p}"));
        }
    }
    let orphans: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if orphans == 0 {
        r.push(Level::Ok, "foreign keys");
    } else {
        r.push(
            Level::Fail,
            format!("{orphans} rows point at events that don't exist"),
        );
    }

    let events = db.events()?;
    let mut bad = 0;
    for ev in &events {
        let name = format!("\"{}\" ({})", ev.title, ev.id);
        if let Some(z) = &ev.tzid
            && z.parse::<chrono_tz::Tz>().is_err()
        {
            r.push(Level::Fail, format!("{name}: unknown time zone {z:?}"));
            bad += 1;
        }
        if ev.all_day && (ev.start % DAY != 0 || ev.end % DAY != 0 || ev.end <= ev.start) {
            r.push(
                Level::Fail,
                format!("{name}: all-day event doesn't span whole days"),
            );
            bad += 1;
        }
        let exceptions = db.exceptions(&ev.id)?;
        if !ev.is_recurring() {
            if !exceptions.is_empty() {
                r.push(
                    Level::Warn,
                    format!(
                        "{name}: doesn't repeat but has {} occurrence changes",
                        exceptions.len()
                    ),
                );
            }
            continue;
        }
        if let Err(e) = recurrence::rule_set(ev) {
            r.push(Level::Fail, format!("{name}: {e:#}"));
            bad += 1;
            continue;
        }
        // a change keyed to a start the series no longer has does nothing
        let stray: Vec<i64> = exceptions
            .iter()
            .map(|x| x.original_start)
            .filter(|&at| {
                recurrence::starts_between(ev, at, at + 1)
                    .map(|s| s.is_empty())
                    .unwrap_or(true)
            })
            .collect();
        if !stray.is_empty() {
            let kinds = exceptions
                .iter()
                .filter(|x| stray.contains(&x.original_start))
                .map(|x| match x.kind {
                    ExceptionKind::Cancelled => "cancelled",
                    ExceptionKind::Modified => "changed",
                })
                .collect::<Vec<_>>();
            r.push(
                Level::Warn,
                format!(
                    "{name}: {} {} occurrence(s) no longer match the series (was its start moved?)",
                    stray.len(),
                    kinds.join("/"),
                ),
            );
        }
    }
    if bad == 0 {
        r.push(Level::Ok, format!("{} events read cleanly", events.len()));
    }

    if config_path.exists() {
        match config::load(config_path) {
            Ok(_) => r.push(Level::Ok, format!("config {}", config_path.display())),
            Err(e) => r.push(Level::Fail, format!("config: {e:#}")),
        }
    } else {
        r.push(
            Level::Ok,
            format!("no config at {}; defaults apply", config_path.display()),
        );
    }

    if db.max_reminder().is_ok_and(|m| m > 0) || events.iter().any(|e| e.reminder.is_some()) {
        let found = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|d| d.join("notify-send").is_file())
        });
        if found {
            r.push(Level::Ok, "notify-send is on PATH for reminders");
        } else {
            r.push(
                Level::Warn,
                "events have reminders but notify-send isn't on PATH (vcal notify needs it)",
            );
        }
    }
    Ok(r.0)
}

/// Where backups go by default: `backups/` next to the database.
pub fn backup_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("backups")
}

const PREFIX: &str = "events-";

/// Writes a consistent copy of the database (`VACUUM INTO`, safe while
/// other processes use it) to `to`, or to a timestamped file in
/// [`backup_dir`]. With `keep`, prunes that directory to the newest `keep`
/// backups. Returns the new file and the ones removed.
pub fn backup(db: &Db, to: Option<&Path>, keep: Option<usize>) -> Result<(PathBuf, Vec<PathBuf>)> {
    let db_path = db
        .path()
        .context("an in-memory database can't be backed up")?;
    let dir = backup_dir(db_path);
    let target = match to {
        Some(p) if p.is_dir() => p.join(stamp_name()),
        Some(p) => p.to_path_buf(),
        None => {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("can't create {}", dir.display()))?;
            dir.join(stamp_name())
        }
    };
    ensure!(
        !target.exists(),
        "{} already exists; vcal won't overwrite it",
        target.display()
    );
    db.conn()
        .execute("VACUUM INTO ?1", [target.to_string_lossy()])
        .with_context(|| format!("can't write {}", target.display()))?;

    let mut removed = Vec::new();
    if let Some(keep) = keep {
        let prune_dir = target.parent().unwrap_or(&dir);
        let mut backups: Vec<PathBuf> = std::fs::read_dir(prune_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(PREFIX) && n.ends_with(".db"))
            })
            .collect();
        // names sort by time
        backups.sort();
        let excess = backups.len().saturating_sub(keep.max(1));
        for old in backups.into_iter().take(excess) {
            std::fs::remove_file(&old)
                .with_context(|| format!("can't remove {}", old.display()))?;
            removed.push(old);
        }
    }
    Ok((target, removed))
}

fn stamp_name() -> String {
    let now = time::datetime_utc(time::now());
    format!("{PREFIX}{}.db", now.format("%Y%m%dT%H%M%SZ"))
}
