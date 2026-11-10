//! `vcal sync`: brings each `[calendars.<name>]` in step. iCalendar feeds
//! (including Proton share links) are read-only copies, replaced on every
//! change; CalDAV calendars (Nextcloud and the like) sync both ways.

mod caldav;
mod feed;
pub mod http;

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;

use crate::config::{CalendarConfig, CalendarKind, Config, Secret};
use crate::db::Db;

/// What one calendar's sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    /// One line: "12 events (3 new, 1 removed)", or the error.
    pub summary: String,
    pub warnings: Vec<String>,
    pub failed: bool,
}

/// What a calendar's sync reports back, and the caching headers to keep.
#[derive(Default)]
struct Done {
    summary: String,
    warnings: Vec<String>,
    etag: Option<String>,
    modified: Option<String>,
}

/// The password a calendar's `password_command` or `password_file` gives.
fn password(cal: &CalendarConfig) -> Result<Option<String>> {
    let at = format!("[calendars.{}]", cal.name);
    let text = match &cal.password {
        Secret::None => return Ok(None),
        Secret::Command(cmd) => {
            let out = std::process::Command::new("sh")
                .args(["-c", cmd])
                .stderr(std::process::Stdio::inherit())
                .output()
                .with_context(|| format!("{at} password_command: can't run sh"))?;
            ensure!(
                out.status.success(),
                "{at} password_command failed ({})",
                out.status
            );
            String::from_utf8(out.stdout).with_context(|| format!("{at} password_command"))?
        }
        Secret::File(path) => std::fs::read_to_string(path)
            .with_context(|| format!("{at} password_file: can't read {}", path.display()))?,
    };
    let line = text.lines().next().unwrap_or("").trim_end().to_string();
    ensure!(!line.is_empty(), "{at}: the password is empty");
    Ok(Some(line))
}

fn sync_one(db: &mut Db, cal: &CalendarConfig, tz: Tz) -> Result<Done> {
    match cal.kind {
        CalendarKind::Ics | CalendarKind::Proton => feed::sync(db, cal, tz),
        CalendarKind::CalDav => {
            let pass = password(cal)?;
            let creds = match (&cal.username, &pass) {
                (Some(u), Some(p)) => Some((u.as_str(), p.as_str())),
                (None, None) => None,
                (None, Some(_)) => bail!("[calendars.{}]: a password needs a username", cal.name),
                (Some(_), None) => bail!(
                    "[calendars.{}]: a username needs password_command or password_file",
                    cal.name
                ),
            };
            caldav::sync(db, cal, &http::Client::new(creds), tz)
        }
    }
}

/// Syncs the calendars named in `only`, or all of them. With all of them,
/// calendars that left the config are cleared out first. One calendar
/// failing doesn't stop the others.
pub fn sync(db: &mut Db, cfg: &Config, only: &[String], tz: Tz) -> Result<Vec<Outcome>> {
    for name in only {
        ensure!(
            cfg.calendar(name).is_some(),
            "no calendar {name:?} in the config"
        );
    }
    let mut out = Vec::new();
    if only.is_empty() {
        let keep: Vec<String> = cfg.calendars.iter().map(|c| c.name.clone()).collect();
        for (name, deleted, kept) in db.forget_calendars(&keep)? {
            let summary = if deleted > 0 {
                format!("no longer configured; removed its {deleted} events")
            } else {
                format!("no longer configured; its {kept} events are local now")
            };
            out.push(Outcome {
                name,
                summary,
                warnings: vec![],
                failed: false,
            });
        }
    }
    for cal in cfg
        .calendars
        .iter()
        .filter(|c| only.is_empty() || only.contains(&c.name))
    {
        db.register_calendar(&cal.name, cal.kind.name(), cal.read_only)?;
        match sync_one(db, cal, tz) {
            Ok(done) => {
                db.finish_sync(
                    &cal.name,
                    None,
                    done.etag.as_deref(),
                    done.modified.as_deref(),
                )?;
                out.push(Outcome {
                    name: cal.name.clone(),
                    summary: done.summary,
                    warnings: done.warnings,
                    failed: false,
                });
            }
            Err(e) => {
                let msg = format!("{e:#}");
                db.finish_sync(&cal.name, Some(&msg), None, None)?;
                out.push(Outcome {
                    name: cal.name.clone(),
                    summary: msg,
                    warnings: vec![],
                    failed: true,
                });
            }
        }
    }
    Ok(out)
}
