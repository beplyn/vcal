//! CRUD and range queries over events and their exceptions.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use chrono_tz::Tz;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};

use super::{Db, recurrence};
use crate::model::{Event, Exception, ExceptionKind, Occurrence, Status};
use crate::time::{self, DAY};

const EVENT_COLS: &str = "id, title, description, category, location, start_ts, end_ts, \
                          all_day, tzid, rrule, status, created_ts, updated_ts, reminder_minutes";
const EXCEPTION_COLS: &str =
    "event_id, original_start_ts, kind, title, description, category, location, start_ts, end_ts";

fn event_from_row(r: &Row) -> rusqlite::Result<Event> {
    let status: String = r.get(10)?;
    Ok(Event {
        id: r.get(0)?,
        title: r.get(1)?,
        description: r.get(2)?,
        category: r.get(3)?,
        location: r.get(4)?,
        start: r.get(5)?,
        end: r.get(6)?,
        all_day: r.get(7)?,
        tzid: r.get(8)?,
        rrule: r.get(9)?,
        // the CHECK constraint keeps this to known values
        status: status.parse().unwrap_or_default(),
        created: r.get(11)?,
        updated: r.get(12)?,
        reminder: r.get(13)?,
    })
}

fn exception_from_row(r: &Row, at: usize) -> rusqlite::Result<Exception> {
    let kind: String = r.get(at + 2)?;
    Ok(Exception {
        event_id: r.get(at)?,
        original_start: r.get(at + 1)?,
        kind: if kind == "cancelled" {
            ExceptionKind::Cancelled
        } else {
            ExceptionKind::Modified
        },
        title: r.get(at + 3)?,
        description: r.get(at + 4)?,
        category: r.get(at + 5)?,
        location: r.get(at + 6)?,
        start: r.get(at + 7)?,
        end: r.get(at + 8)?,
    })
}

/// Validates and canonicalises an event before it is written; returns its
/// `series_end_ts`.
fn prepare(ev: &mut Event) -> Result<Option<i64>> {
    ev.title = ev.title.trim().to_string();
    ensure!(!ev.title.is_empty(), "an event needs a title");
    ensure!(ev.end >= ev.start, "event ends before it starts");
    if ev.all_day {
        ensure!(
            ev.start % DAY == 0 && ev.end % DAY == 0 && ev.end > ev.start,
            "all-day events must span whole floating days"
        );
        ev.tzid = None;
    } else if ev.tzid.is_none() {
        ev.tzid = Some(time::local_tz().name().to_string());
    }
    if let Some(z) = &ev.tzid {
        z.parse::<Tz>()
            .map_err(|_| anyhow::anyhow!("unknown time zone {z:?}"))?;
    }
    ev.category = ev
        .category
        .take()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    ev.location = ev
        .location
        .take()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty());
    ensure!(
        ev.reminder.is_none_or(|m| m >= 0),
        "a reminder can't come after the start"
    );
    if let Some(rule) = ev.rrule.take().filter(|r| !r.trim().is_empty()) {
        ev.rrule = Some(recurrence::normalize(
            &rule,
            ev.all_day,
            recurrence::zone(ev),
        )?);
    }
    recurrence::series_end(ev)
}

/// Inserts, or with `upsert` replaces, an event (keeping its creation time).
fn write_event(conn: &Connection, mut ev: Event, upsert: bool) -> Result<Event> {
    let series_end = prepare(&mut ev)?;
    ev.updated = time::now();
    let conflict = if upsert {
        "ON CONFLICT (id) DO UPDATE SET title = excluded.title, description = excluded.description,
         category = excluded.category, location = excluded.location, start_ts = excluded.start_ts,
         end_ts = excluded.end_ts, all_day = excluded.all_day, tzid = excluded.tzid,
         rrule = excluded.rrule, series_end_ts = excluded.series_end_ts, status = excluded.status,
         updated_ts = excluded.updated_ts, reminder_minutes = excluded.reminder_minutes"
    } else {
        ""
    };
    conn.execute(
        &format!(
            "INSERT INTO events ({EVENT_COLS}, series_end_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15) {conflict}"
        ),
        params![
            ev.id,
            ev.title,
            ev.description,
            ev.category,
            ev.location,
            ev.start,
            ev.end,
            ev.all_day,
            ev.tzid,
            ev.rrule,
            ev.status.as_str(),
            ev.created,
            ev.updated,
            ev.reminder,
            series_end,
        ],
    )
    .with_context(|| format!("can't save event {}", ev.id))?;
    if ev.rrule.is_none() {
        // per-occurrence changes mean nothing once an event stops repeating
        conn.execute("DELETE FROM event_exceptions WHERE event_id = ?1", [&ev.id])?;
    }
    note_span(conn, ev.duration())?;
    Ok(ev)
}

/// Remembers the longest event duration so range queries can bound their
/// scan of the start index. It only ever grows, which keeps it correct.
fn note_span(conn: &Connection, span: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO metadata (key, value) VALUES ('max_span', ?1)
         ON CONFLICT (key) DO UPDATE
         SET value = MAX(CAST(value AS INTEGER), CAST(excluded.value AS INTEGER))",
        [span],
    )?;
    Ok(())
}

fn get_event(conn: &Connection, id: &str) -> Result<Option<Event>> {
    Ok(conn
        .query_row(
            &format!("SELECT {EVENT_COLS} FROM events WHERE id = ?1"),
            [id],
            event_from_row,
        )
        .optional()?)
}

fn write_exception(conn: &Connection, mut exc: Exception) -> Result<()> {
    let Some(ev) = get_event(conn, &exc.event_id)? else {
        bail!("no event with id {}", exc.event_id);
    };
    ensure!(
        ev.is_recurring(),
        "event {} doesn't repeat; edit it directly",
        ev.id
    );
    match (exc.start, exc.end) {
        (Some(s), None) => exc.end = Some(s + ev.duration()),
        (None, Some(_)) => bail!("a moved occurrence needs a start"),
        (Some(s), Some(e)) => ensure!(e >= s, "occurrence ends before it starts"),
        (None, None) => {}
    }
    if let (Some(s), Some(e)) = (exc.start, exc.end) {
        note_span(conn, e - s)?;
    }
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO event_exceptions ({EXCEPTION_COLS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
        ),
        params![
            exc.event_id,
            exc.original_start,
            exc.kind.as_str(),
            exc.title,
            exc.description,
            exc.category,
            exc.location,
            exc.start,
            exc.end,
        ],
    )?;
    Ok(())
}

/// Half-open overlap; zero-length events count when they sit inside the window.
fn overlaps(start: i64, end: i64, from: i64, to: i64) -> bool {
    start < to && (end > from || (start == end && start >= from))
}

fn occurrence(
    ev: &Event,
    original: Option<i64>,
    start: i64,
    exc: Option<&Exception>,
) -> Occurrence {
    let mut occ = Occurrence {
        event_id: ev.id.clone(),
        original_start: original,
        title: ev.title.clone(),
        description: ev.description.clone(),
        category: ev.category.clone(),
        location: ev.location.clone(),
        start,
        end: start + ev.duration(),
        all_day: ev.all_day,
        status: ev.status,
        reminder: ev.reminder,
        modified: false,
    };
    if let Some(x) = exc.filter(|x| x.kind == ExceptionKind::Modified) {
        occ.modified = true;
        if let Some(t) = &x.title {
            occ.title = t.clone();
        }
        if let Some(d) = &x.description {
            occ.description = d.clone();
        }
        if x.category.is_some() {
            occ.category = x.category.clone();
        }
        if x.location.is_some() {
            occ.location = x.location.clone();
        }
        if let (Some(s), Some(e)) = (x.start, x.end) {
            occ.start = s;
            occ.end = e;
        }
    }
    occ
}

impl Db {
    /// Stores a new event and returns it as saved (trimmed, rule normalised,
    /// zone filled in).
    pub fn insert(&mut self, ev: Event) -> Result<Event> {
        self.write(|tx| write_event(tx, ev, false))
    }

    /// Replaces an existing event's fields.
    pub fn update(&mut self, ev: Event) -> Result<Event> {
        self.write(|tx| {
            ensure!(
                get_event(tx, &ev.id)?.is_some(),
                "no event with id {}",
                ev.id
            );
            write_event(tx, ev, true)
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Event>> {
        get_event(&self.conn, id)
    }

    /// Every stored event, oldest start first.
    pub fn events(&self) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLS} FROM events ORDER BY start_ts, id"
        ))?;
        let rows = stmt.query_map([], event_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Events with their exceptions, as `ics::write::calendar` takes them.
    pub fn with_exceptions(&self, events: Vec<Event>) -> Result<Vec<(Event, Vec<Exception>)>> {
        events
            .into_iter()
            .map(|e| {
                let x = self.exceptions(&e.id)?;
                Ok((e, x))
            })
            .collect()
    }

    /// Deletes an event and, by cascade, its exceptions. False if it didn't exist.
    pub fn delete(&mut self, id: &str) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM events WHERE id = ?1", [id])? > 0))
    }

    /// Records a cancelled or modified occurrence of a recurring event.
    pub fn set_exception(&mut self, exc: Exception) -> Result<()> {
        self.write(|tx| write_exception(tx, exc))
    }

    pub fn cancel_occurrence(&mut self, id: &str, original_start: i64) -> Result<()> {
        self.set_exception(Exception::cancelled(id, original_start))
    }

    pub fn exceptions(&self, id: &str) -> Result<Vec<Exception>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {EXCEPTION_COLS} FROM event_exceptions WHERE event_id = ?1
             ORDER BY original_start_ts"
        ))?;
        let rows = stmt.query_map([id], |r| exception_from_row(r, 0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Upserts events (by id) then their exceptions in one transaction. The
    /// file is the source of truth for the events in it: their earlier
    /// exceptions are replaced, so an EXDATE dropped from a feed un-cancels
    /// that occurrence. Exceptions whose series is missing or doesn't repeat
    /// are skipped. Returns (events written, exceptions written).
    pub fn import(
        &mut self,
        events: Vec<Event>,
        exceptions: Vec<Exception>,
    ) -> Result<(usize, usize)> {
        self.write(|tx| {
            let n_events = events.len();
            for ev in events {
                tx.execute("DELETE FROM event_exceptions WHERE event_id = ?1", [&ev.id])?;
                write_event(tx, ev, true)?;
            }
            let mut n_exc = 0;
            for exc in exceptions {
                let recurring = get_event(tx, &exc.event_id)?.is_some_and(|e| e.is_recurring());
                if recurring {
                    write_exception(tx, exc)?;
                    n_exc += 1;
                }
            }
            Ok((n_events, n_exc))
        })
    }

    /// The longest reminder lead time in minutes, 0 when there are none.
    pub fn max_reminder(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(reminder_minutes), 0) FROM events",
            [],
            |r| r.get(0),
        )?)
    }

    /// Whether the reminder for the occurrence of `event_id` starting at
    /// `start` was already sent.
    pub fn was_notified(&self, event_id: &str, start: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM notified WHERE event_id = ?1 AND start_ts = ?2",
                params![event_id, start],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Records sent reminders and forgets ones for occurrences that started
    /// before `forget_before`.
    pub fn mark_notified(
        &mut self,
        sent: &[(String, i64)],
        now: i64,
        forget_before: i64,
    ) -> Result<()> {
        self.write(|tx| {
            for (id, start) in sent {
                tx.execute(
                    "INSERT OR IGNORE INTO notified (event_id, start_ts, sent_ts) VALUES (?1, ?2, ?3)",
                    params![id, start, now],
                )?;
            }
            tx.execute("DELETE FROM notified WHERE start_ts < ?1", [forget_before])?;
            Ok(())
        })
    }

    /// Every occurrence overlapping `[from, to)`, recurring series expanded and
    /// exceptions applied. All-day events are matched by the dates the window
    /// covers in `tz`. Sorted by start, all-day first on a given day.
    pub fn occurrences(
        &self,
        from: i64,
        to: i64,
        tz: Tz,
        include_cancelled: bool,
    ) -> Result<Vec<Occurrence>> {
        ensure!(to >= from, "query window ends before it starts");
        let span: i64 = self
            .conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM metadata WHERE key = 'max_span'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let (ffrom, fto) = (time::to_floating(from, tz), time::to_floating(to, tz));
        let window = |all_day: bool| if all_day { (ffrom, fto) } else { (from, to) };
        let keep = |s: Status| include_cancelled || s != Status::Cancelled;
        let mut out = Vec::new();

        // Single events: a bounded scan of (all_day, start_ts).
        let mut single = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLS} FROM events
             WHERE rrule IS NULL AND all_day = :all_day
               AND start_ts >= :lo AND start_ts < :hi"
        ))?;
        for all_day in [false, true] {
            let (lo, hi) = window(all_day);
            let rows = single.query_map(
                named_params! { ":all_day": all_day, ":lo": lo - span, ":hi": hi },
                event_from_row,
            )?;
            for ev in rows {
                let ev = ev?;
                if keep(ev.status) && overlaps(ev.start, ev.end, lo, hi) {
                    out.push(occurrence(&ev, None, ev.start, None));
                }
            }
        }

        // Recurring series still running in the window. Floating and real
        // bounds differ by at most a zone offset, so pad by two days.
        let (lo, hi) = (from.min(ffrom) - 2 * DAY, to.max(fto) + 2 * DAY);
        let mut series = self.conn.prepare_cached(&format!(
            "SELECT {EVENT_COLS} FROM events
             WHERE rrule IS NOT NULL AND start_ts < :hi
               AND (series_end_ts IS NULL OR series_end_ts > :lo)"
        ))?;
        let series: Vec<Event> = series
            .query_map(named_params! { ":lo": lo, ":hi": hi }, event_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        let mut handled = HashSet::new();
        for ev in series.iter().filter(|e| keep(e.status)) {
            let (wlo, whi) = window(ev.all_day);
            let exceptions: HashMap<i64, Exception> = self
                .exceptions(&ev.id)?
                .into_iter()
                .map(|x| (x.original_start, x))
                .collect();
            let starts = recurrence::starts_between(ev, wlo - ev.duration(), whi)
                .with_context(|| format!("expanding event {}", ev.id))?;
            for start in starts {
                handled.insert((ev.id.clone(), start));
                let exc = exceptions.get(&start);
                if exc.is_some_and(|x| x.kind == ExceptionKind::Cancelled) {
                    continue;
                }
                let occ = occurrence(ev, Some(start), start, exc);
                if overlaps(occ.start, occ.end, wlo, whi) {
                    out.push(occ);
                }
            }
        }

        // Occurrences moved into the window from outside it.
        let mut moved = self.conn.prepare_cached(&format!(
            "SELECT {}, {} FROM event_exceptions x JOIN events e ON e.id = x.event_id
             WHERE x.kind = 'modified' AND x.start_ts IS NOT NULL AND e.rrule IS NOT NULL
               AND x.start_ts >= :lo - :span AND x.start_ts < :hi",
            EVENT_COLS
                .split(", ")
                .map(|c| format!("e.{c}"))
                .collect::<Vec<_>>()
                .join(", "),
            EXCEPTION_COLS
                .split(", ")
                .map(|c| format!("x.{c}"))
                .collect::<Vec<_>>()
                .join(", "),
        ))?;
        let rows = moved.query_map(named_params! { ":lo": lo, ":hi": hi, ":span": span }, |r| {
            Ok((event_from_row(r)?, exception_from_row(r, 14)?))
        })?;
        for row in rows {
            let (ev, exc) = row?;
            if !keep(ev.status) || handled.contains(&(ev.id.clone(), exc.original_start)) {
                continue;
            }
            let (wlo, whi) = window(ev.all_day);
            let occ = occurrence(
                &ev,
                Some(exc.original_start),
                exc.original_start,
                Some(&exc),
            );
            if overlaps(occ.start, occ.end, wlo, whi) {
                out.push(occ);
            }
        }

        let sort_key = |o: &Occurrence| {
            let at = if o.all_day {
                time::local_midnight(time::floating_date(o.start), tz)
            } else {
                o.start
            };
            (at, !o.all_day, o.end)
        };
        out.sort_by(|a, b| {
            sort_key(a)
                .cmp(&sort_key(b))
                .then_with(|| a.title.cmp(&b.title))
        });
        Ok(out)
    }
}
