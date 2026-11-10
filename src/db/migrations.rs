//! Forward-only schema migrations tracked in `PRAGMA user_version`.
//! Append new steps; never edit one that has shipped.

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, TransactionBehavior};

const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE events (
        id            TEXT PRIMARY KEY,
        title         TEXT NOT NULL,
        description   TEXT NOT NULL DEFAULT '',
        category      TEXT,
        location      TEXT,
        start_ts      INTEGER NOT NULL,
        end_ts        INTEGER NOT NULL,
        all_day       INTEGER NOT NULL DEFAULT 0 CHECK (all_day IN (0, 1)),
        tzid          TEXT,
        rrule         TEXT,
        series_end_ts INTEGER,
        status        TEXT NOT NULL DEFAULT 'confirmed'
                      CHECK (status IN ('confirmed', 'tentative', 'cancelled')),
        created_ts    INTEGER NOT NULL,
        updated_ts    INTEGER NOT NULL,
        CHECK (end_ts >= start_ts)
    );
    CREATE INDEX idx_events_range  ON events (start_ts, end_ts);
    CREATE INDEX idx_events_allday ON events (all_day, start_ts);
    CREATE INDEX idx_events_series ON events (series_end_ts) WHERE rrule IS NOT NULL;

    CREATE TABLE event_exceptions (
        event_id          TEXT NOT NULL REFERENCES events (id) ON DELETE CASCADE,
        original_start_ts INTEGER NOT NULL,
        kind              TEXT NOT NULL CHECK (kind IN ('cancelled', 'modified')),
        title             TEXT,
        description       TEXT,
        category          TEXT,
        location          TEXT,
        start_ts          INTEGER,
        end_ts            INTEGER,
        PRIMARY KEY (event_id, original_start_ts),
        CHECK (end_ts IS NULL OR start_ts IS NULL OR end_ts >= start_ts)
    );
    CREATE INDEX idx_exceptions_moved ON event_exceptions (start_ts) WHERE start_ts IS NOT NULL;

    CREATE TABLE metadata (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // 2: reminders, and which occurrences were already notified
    r#"
    ALTER TABLE events ADD COLUMN reminder_minutes INTEGER
        CHECK (reminder_minutes IS NULL OR reminder_minutes >= 0);

    CREATE TABLE notified (
        event_id TEXT NOT NULL REFERENCES events (id) ON DELETE CASCADE,
        start_ts INTEGER NOT NULL,
        sent_ts  INTEGER NOT NULL,
        PRIMARY KEY (event_id, start_ts)
    );
    "#,
    // 3: calendars synced from elsewhere (feeds, CalDAV) and what was synced
    r#"
    ALTER TABLE events ADD COLUMN calendar TEXT;
    CREATE INDEX idx_events_calendar ON events (calendar) WHERE calendar IS NOT NULL;

    -- the [calendars] the last sync saw; read_only ones refuse local edits
    CREATE TABLE calendars (
        name         TEXT PRIMARY KEY,
        kind         TEXT NOT NULL,
        read_only    INTEGER NOT NULL DEFAULT 0 CHECK (read_only IN (0, 1)),
        -- a feed's ETag / Last-Modified, for conditional requests
        etag         TEXT,
        modified     TEXT,
        last_sync_ts INTEGER,
        last_error   TEXT
    );

    -- one row per event stored on a CalDAV server: where, its ETag, and the
    -- event's updated_ts when it was last in step with the server. No
    -- foreign key: a row whose event is gone is a deletion to push.
    CREATE TABLE sync_items (
        calendar  TEXT NOT NULL,
        event_id  TEXT NOT NULL,
        href      TEXT NOT NULL,
        etag      TEXT,
        synced_ts INTEGER NOT NULL,
        PRIMARY KEY (calendar, event_id)
    );
    CREATE UNIQUE INDEX idx_sync_items_href ON sync_items (calendar, href);
    "#,
];

pub fn latest() -> i64 {
    MIGRATIONS.len() as i64
}

/// Brings the schema up to date; a no-op when it already is.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > latest() {
        bail!(
            "database schema v{current} is newer than this vcal understands (v{}); upgrade vcal",
            latest()
        );
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let version = i as i64 + 1;
        tx.execute_batch(sql)
            .with_context(|| format!("migration {version} failed"))?;
        tx.pragma_update(None, "user_version", version)?;
        tx.execute(
            "INSERT INTO metadata (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [version.to_string()],
        )?;
    }
    tx.commit()?;
    Ok(())
}
