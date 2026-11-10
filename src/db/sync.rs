//! Storage for synced calendars: which ones exist, their feed caching
//! headers and last result, and for CalDAV, where each event lives on the
//! server. These writes skip the read-only guard: they are the sync.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params};

use super::Db;
use super::repo::{EVENT_COLS, event_from_row, get_event, write_event, write_exception};
use crate::model::{Event, Exception};
use crate::time;

/// A calendar as the database remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarInfo {
    pub name: String,
    pub kind: String,
    pub read_only: bool,
    pub etag: Option<String>,
    pub modified: Option<String>,
    pub last_sync: Option<i64>,
    pub last_error: Option<String>,
    pub events: i64,
}

/// One event stored on a CalDAV server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncItem {
    pub event_id: String,
    pub href: String,
    pub etag: Option<String>,
    /// The event's `updated_ts` when it last matched the server.
    pub synced: i64,
}

/// Upserts events into `calendar`, replacing each one's exceptions with
/// the given ones (exceptions of other series are ignored).
fn store(
    tx: &Transaction,
    calendar: &str,
    events: Vec<Event>,
    exceptions: &[Exception],
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for mut ev in events {
        ev.calendar = Some(calendar.to_string());
        if let Some(old) = get_event(tx, &ev.id)? {
            ev.created = old.created;
        }
        tx.execute("DELETE FROM event_exceptions WHERE event_id = ?1", [&ev.id])?;
        let saved = write_event(tx, ev, true)?;
        ids.push(saved.id);
    }
    for x in exceptions {
        if ids.contains(&x.event_id)
            && get_event(tx, &x.event_id)?.is_some_and(|e| e.is_recurring())
        {
            write_exception(tx, x.clone())?;
        }
    }
    Ok(ids)
}

fn updated_ts(tx: &Transaction, id: &str) -> Result<i64> {
    Ok(
        tx.query_row("SELECT updated_ts FROM events WHERE id = ?1", [id], |r| {
            r.get(0)
        })?,
    )
}

impl Db {
    /// Records that `name` is configured, as a `kind` calendar.
    pub fn register_calendar(&mut self, name: &str, kind: &str, read_only: bool) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO calendars (name, kind, read_only) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO UPDATE SET kind = excluded.kind,
                 read_only = excluded.read_only",
                params![name, kind, read_only],
            )?;
            Ok(())
        })
    }

    pub fn calendars(&self) -> Result<Vec<CalendarInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.name, c.kind, c.read_only, c.etag, c.modified, c.last_sync_ts,
                    c.last_error, (SELECT COUNT(*) FROM events e WHERE e.calendar = c.name)
             FROM calendars c ORDER BY c.name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CalendarInfo {
                name: r.get(0)?,
                kind: r.get(1)?,
                read_only: r.get(2)?,
                etag: r.get(3)?,
                modified: r.get(4)?,
                last_sync: r.get(5)?,
                last_error: r.get(6)?,
                events: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn calendar(&self, name: &str) -> Result<Option<CalendarInfo>> {
        Ok(self.calendars()?.into_iter().find(|c| c.name == name))
    }

    /// Notes how a sync of `name` went; `error` is `None` on success.
    pub fn finish_sync(
        &mut self,
        name: &str,
        error: Option<&str>,
        etag: Option<&str>,
        modified: Option<&str>,
    ) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE calendars SET last_sync_ts = ?2, last_error = ?3,
                 etag = COALESCE(?4, etag), modified = COALESCE(?5, modified)
                 WHERE name = ?1",
                params![name, time::now(), error, etag, modified],
            )?;
            Ok(())
        })
    }

    /// Calendars that left the config: a feed's events go (they're copies),
    /// a CalDAV calendar's become local events (they may hold changes that
    /// never reached the server). Returns (calendar, events deleted, events kept).
    pub fn forget_calendars(&mut self, keep: &[String]) -> Result<Vec<(String, usize, usize)>> {
        let gone: Vec<CalendarInfo> = self
            .calendars()?
            .into_iter()
            .filter(|c| !keep.contains(&c.name))
            .collect();
        if gone.is_empty() {
            return Ok(vec![]);
        }
        self.write(|tx| {
            let mut out = Vec::new();
            for c in gone {
                let (deleted, kept) = if c.read_only {
                    let n = tx.execute("DELETE FROM events WHERE calendar = ?1", [&c.name])?;
                    (n, 0)
                } else {
                    let n = tx.execute(
                        "UPDATE events SET calendar = NULL WHERE calendar = ?1",
                        [&c.name],
                    )?;
                    (0, n)
                };
                tx.execute("DELETE FROM sync_items WHERE calendar = ?1", [&c.name])?;
                tx.execute("DELETE FROM calendars WHERE name = ?1", [&c.name])?;
                out.push((c.name, deleted, kept));
            }
            Ok(out)
        })
    }

    /// Makes `calendar`'s events exactly the given ones: upserts them and
    /// deletes the calendar's other events. Returns (new, removed).
    pub fn replace_calendar(
        &mut self,
        calendar: &str,
        events: Vec<Event>,
        exceptions: Vec<Exception>,
    ) -> Result<(usize, usize)> {
        self.write(|tx| {
            let before: HashSet<String> = {
                let mut stmt = tx.prepare("SELECT id FROM events WHERE calendar = ?1")?;
                let rows = stmt.query_map([calendar], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let ids: HashSet<String> = store(tx, calendar, events, &exceptions)?
                .into_iter()
                .collect();
            let mut removed = 0;
            for old in before.difference(&ids) {
                removed += tx.execute("DELETE FROM events WHERE id = ?1", [old])?;
            }
            Ok((ids.difference(&before).count(), removed))
        })
    }

    /// Events assigned to `calendar`.
    pub fn events_in(&self, calendar: &str) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM events WHERE calendar = ?1 ORDER BY start_ts"
        ))?;
        let rows = stmt.query_map([calendar], event_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn sync_items(&self, calendar: &str) -> Result<Vec<SyncItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, href, etag, synced_ts FROM sync_items WHERE calendar = ?1
             ORDER BY href",
        )?;
        let rows = stmt.query_map([calendar], |r| {
            Ok(SyncItem {
                event_id: r.get(0)?,
                href: r.get(1)?,
                etag: r.get(2)?,
                synced: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// After a successful upload: the event at `href` now has `etag` and
    /// matches the server as it is locally.
    pub fn mark_pushed(
        &mut self,
        calendar: &str,
        event_id: &str,
        href: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        self.write(|tx| {
            let synced = updated_ts(tx, event_id)?;
            tx.execute(
                "INSERT INTO sync_items (calendar, event_id, href, etag, synced_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (calendar, event_id) DO UPDATE SET href = excluded.href,
                 etag = excluded.etag, synced_ts = excluded.synced_ts",
                params![calendar, event_id, href, etag, synced],
            )?;
            Ok(())
        })
    }

    pub fn forget_item(&mut self, calendar: &str, event_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM sync_items WHERE calendar = ?1 AND event_id = ?2",
                params![calendar, event_id],
            )?;
            Ok(())
        })
    }

    /// Stores what the server has at `href` (one event and its overrides)
    /// and remembers it as in step. An event that `href` used to hold under
    /// another id is deleted. Returns the stored event's id.
    pub fn store_remote(
        &mut self,
        calendar: &str,
        href: &str,
        etag: Option<&str>,
        event: Event,
        exceptions: Vec<Exception>,
    ) -> Result<String> {
        self.write(|tx| {
            let previous: Option<String> = tx
                .query_row(
                    "SELECT event_id FROM sync_items WHERE calendar = ?1 AND href = ?2",
                    params![calendar, href],
                    |r| r.get(0),
                )
                .optional()?;
            let id = store(tx, calendar, vec![event], &exceptions)?.remove(0);
            if let Some(old) = previous.filter(|old| *old != id) {
                tx.execute("DELETE FROM events WHERE id = ?1", [&old])?;
                tx.execute(
                    "DELETE FROM sync_items WHERE calendar = ?1 AND event_id = ?2",
                    params![calendar, old],
                )?;
            }
            let synced = updated_ts(tx, &id)?;
            tx.execute(
                "INSERT INTO sync_items (calendar, event_id, href, etag, synced_ts)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (calendar, event_id) DO UPDATE SET href = excluded.href,
                 etag = excluded.etag, synced_ts = excluded.synced_ts",
                params![calendar, id, href, etag, synced],
            )?;
            Ok(id)
        })
    }

    /// The server no longer has this event: drop it and its sync row.
    pub fn delete_remote(&mut self, calendar: &str, event_id: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "DELETE FROM events WHERE id = ?1 AND calendar = ?2",
                params![event_id, calendar],
            )?;
            tx.execute(
                "DELETE FROM sync_items WHERE calendar = ?1 AND event_id = ?2",
                params![calendar, event_id],
            )?;
            Ok(())
        })
    }
}
