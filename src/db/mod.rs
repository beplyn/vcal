//! SQLite storage: one WAL-mode database shared by the TUI, the CLI and the
//! desktop shell. Every committed write also touches `<db>.changed`, which
//! watchers can follow instead of the WAL file (SQLite truncates and deletes
//! that on checkpoint).

mod migrations;
pub mod recurrence;
mod repo;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction, TransactionBehavior};

pub use migrations::latest as schema_version;
pub use repo::Snapshot;

pub struct Db {
    conn: Connection,
    path: Option<PathBuf>,
}

/// `$XDG_DATA_HOME/calendar/events.db`, else `~/.local/share/calendar/events.db`.
pub fn default_path() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("calendar").join("events.db")
}

/// The file touched after each write, next to the database.
pub fn sentinel_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(".changed");
    db.with_file_name(name)
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("can't create {}", dir.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("can't open {}", path.display()))?;
        let db = Self::setup(conn, Some(path.to_path_buf()))?;
        // Watchers (Quickshell) can only follow a file that exists; create it
        // without content so the first real write still reads as a change.
        let sentinel = sentinel_path(path);
        if !sentinel.exists() {
            let _ = std::fs::write(&sentinel, "");
        }
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::setup(Connection::open_in_memory()?, None)
    }

    fn setup(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(Duration::from_millis(5000))?;
        if path.is_some() {
            let mode: String =
                conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
            anyhow::ensure!(
                mode.eq_ignore_ascii_case("wal"),
                "couldn't enable WAL (got {mode})"
            );
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrate(&mut conn)?;
        Ok(Db { conn, path })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Runs `f` in an IMMEDIATE transaction (so lock waits go through the busy
    /// timeout instead of failing on upgrade), commits, then touches the sentinel.
    fn write<T>(&mut self, f: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        if let Some(path) = &self.path {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            // Best effort: the data is committed either way, watchers just miss a beat.
            let _ = std::fs::write(sentinel_path(path), format!("{}\n", stamp.as_nanos()));
        }
        Ok(out)
    }
}
