//! Domain types shared by storage, the CLI and (later) the TUI.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::time;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Confirmed,
    Tentative,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Confirmed => "confirmed",
            Status::Tentative => "tentative",
            Status::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Status {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "confirmed" => Status::Confirmed,
            "tentative" => Status::Tentative,
            "cancelled" | "canceled" => Status::Cancelled,
            _ => bail!("unknown status {s:?} (confirmed, tentative, cancelled)"),
        })
    }
}

/// A stored event: a single occurrence, or the template of a recurring series.
///
/// `start`/`end` are UTC epoch seconds, `end` exclusive. For all-day events
/// they are floating dates (see [`time::floating`]), so a one-day event spans
/// exactly [`time::DAY`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: Option<String>,
    pub location: Option<String>,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// IANA zone the recurrence rule is expanded in; `None` for all-day events.
    pub tzid: Option<String>,
    /// RFC 5545 RRULE value, without the `RRULE:` prefix.
    pub rrule: Option<String>,
    pub status: Status,
    pub created: i64,
    pub updated: i64,
}

impl Event {
    /// A new confirmed event with a fresh time-ordered id.
    pub fn new(title: impl Into<String>, start: i64, end: i64) -> Self {
        let now = time::now();
        Event {
            id: uuid::Uuid::now_v7().to_string(),
            title: title.into(),
            description: String::new(),
            category: None,
            location: None,
            start,
            end,
            all_day: false,
            tzid: None,
            rrule: None,
            status: Status::Confirmed,
            created: now,
            updated: now,
        }
    }

    pub fn duration(&self) -> i64 {
        self.end - self.start
    }

    pub fn is_recurring(&self) -> bool {
        self.rrule.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExceptionKind {
    Cancelled,
    Modified,
}

impl ExceptionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ExceptionKind::Cancelled => "cancelled",
            ExceptionKind::Modified => "modified",
        }
    }
}

/// A change to one occurrence of a series, keyed by the occurrence's
/// original start (its RECURRENCE-ID). `None` fields inherit from the series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exception {
    pub event_id: String,
    pub original_start: i64,
    pub kind: ExceptionKind,
    pub title: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub location: Option<String>,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl Exception {
    pub fn cancelled(event_id: impl Into<String>, original_start: i64) -> Self {
        Exception {
            event_id: event_id.into(),
            original_start,
            kind: ExceptionKind::Cancelled,
            title: None,
            description: None,
            category: None,
            location: None,
            start: None,
            end: None,
        }
    }

    pub fn modified(event_id: impl Into<String>, original_start: i64) -> Self {
        Exception {
            kind: ExceptionKind::Modified,
            ..Exception::cancelled(event_id, original_start)
        }
    }
}

/// One concrete appearance of an event inside a queried window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub event_id: String,
    /// RECURRENCE-ID for recurring series; `None` for single events.
    pub original_start: Option<i64>,
    pub title: String,
    pub description: String,
    pub category: Option<String>,
    pub location: Option<String>,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub status: Status,
    pub modified: bool,
}

impl Occurrence {
    pub fn recurring(&self) -> bool {
        self.original_start.is_some()
    }
}
