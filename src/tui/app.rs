//! Application state and the update step: keys in, state changes and
//! database writes out. Rendering lives in `ui`; nothing here touches the
//! terminal, so the whole flow is testable with plain key sequences.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Months, NaiveDate, NaiveTime, Timelike};
use chrono_tz::Tz;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::form::{Form, Submit, TextInput};
use super::keymap::{self, Action, Key, KeyMode, Keymap, Resolved};
use super::theme::Theme;
use crate::config::{self, Config};
use crate::db::{self, Db, Snapshot};
use crate::model::{Event, Exception, Occurrence, Status};
use crate::settings::{Settings, View};
use crate::time::{self, DAY};

/// Counts are capped so `99999j` can't stall the UI.
const MAX_COUNT: u32 = 999;

/// How many days the agenda loads, from the start of the cursor's week.
pub const AGENDA_DAYS: i64 = 56;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Visual,
    Insert,
    Command,
    Search,
    Prompt,
    Help,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal | Mode::Help => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Prompt => "CONFIRM",
        }
    }
}

/// Whether the cursor is on the calendar grid or on the day's event list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Grid,
    Events,
}

/// A cursor position: a day and a time slot within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub date: NaiveDate,
    pub slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeAction {
    Delete,
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    /// Confirm deleting these (a recurring entry cancels just that occurrence).
    Delete(Vec<Occurrence>),
    /// "This occurrence or the whole series?"
    Scope { occ: Occurrence, then: ScopeAction },
    /// A visual selection that includes repeating events: "just these
    /// occurrences, or every occurrence of their series?"
    SelectionScope(Vec<Occurrence>),
}

/// How many changes `u` can go back.
const UNDO_DEPTH: usize = 100;

/// Events as snapshotted, by id; `None` for one that didn't exist.
type States = Vec<(String, Option<Snapshot>)>;

/// One undoable change: reverting it means checking the events are still
/// as `after` left them, then putting `before` back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    before: States,
    after: States,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub error: bool,
}

pub struct App {
    pub db: Db,
    pub tz: Tz,
    pub settings: Settings,
    pub theme: Theme,
    pub keymap: Keymap,
    pub now: i64,
    pub view: View,
    pub focus: Focus,
    pub cursor: NaiveDate,
    pub slot: u32,
    /// First slot row shown in week/day views.
    pub scroll: u32,
    /// Slot rows that fit, recorded by the last render.
    pub viewport_slots: u32,
    pub event_idx: usize,
    pub mode: Mode,
    pub anchor: Option<Point>,
    pub pending: Vec<Key>,
    /// A count typed before a command, as in `3j`.
    pub count: Option<u32>,
    /// The `:` / `/` line.
    pub line: TextInput,
    pub form: Option<Form>,
    pub prompt: Option<Prompt>,
    pub filter: Option<String>,
    pub yanked: Vec<Occurrence>,
    /// Changes `u` can revert, newest last, and ones `<C-r>` can redo.
    pub undo: Vec<Change>,
    pub redo: Vec<Change>,
    /// `[calendars]` from the config: for the dialog, `:add`, paste and `:sync`.
    pub calendars: Vec<config::CalendarConfig>,
    pub message: Option<Message>,
    pub quit: bool,
    cache: Vec<Occurrence>,
    loaded: Option<(NaiveDate, NaiveDate)>,
    stamp: Option<String>,
    /// config.toml being followed for live reloads, and its last-seen version.
    config_path: Option<PathBuf>,
    config_stamp: config::Stamp,
    database_path: Option<PathBuf>,
}

/// Gives an occurrence new times: the event itself when it doesn't repeat,
/// otherwise just this occurrence, keeping its other changes.
fn set_span(db: &mut Db, o: &Occurrence, start: i64, end: i64) -> Result<()> {
    match o.original_start {
        Some(at) => {
            let x = db
                .exceptions(&o.event_id)?
                .into_iter()
                .find(|e| e.original_start == at && o.modified);
            let mut x = x.unwrap_or_else(|| Exception::modified(&o.event_id, at));
            x.start = Some(start);
            x.end = Some(end);
            db.set_exception(x)
        }
        None => {
            let mut ev = db.get(&o.event_id)?.context("event vanished")?;
            ev.start = start;
            ev.end = end;
            db.update(ev).map(|_| ())
        }
    }
}

/// A path typed on the command line, with a leading `~/` expanded.
fn expand_home(arg: &str) -> PathBuf {
    match arg.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest),
        None => PathBuf::from(arg),
    }
}

fn matches(o: &Occurrence, needle: &str) -> bool {
    o.matches(needle)
}

impl App {
    pub fn new(db: Db, tz: Tz, settings: Settings, now: i64) -> Self {
        let today = time::local_date(now, tz);
        let mut app = App {
            db,
            tz,
            view: settings.start_view,
            settings,
            theme: Theme::default(),
            keymap: Keymap::default(),
            now,
            focus: Focus::Grid,
            cursor: today,
            slot: 0,
            scroll: 0,
            viewport_slots: 24,
            event_idx: 0,
            mode: Mode::Normal,
            anchor: None,
            pending: Vec::new(),
            count: None,
            line: TextInput::default(),
            form: None,
            prompt: None,
            filter: None,
            yanked: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            calendars: Vec::new(),
            message: None,
            quit: false,
            cache: Vec::new(),
            loaded: None,
            stamp: None,
            config_path: None,
            config_stamp: None,
            database_path: None,
        };
        app.stamp = app.read_stamp();
        app.goto_today();
        app.refresh();
        app
    }

    /// Switches to new settings, theme and keys, keeping the cursor where
    /// it is (clamped into the new grid).
    pub fn apply_config(&mut self, cfg: Config) {
        // slots are counted in the old granularity; carry over the time of day
        let (minutes, top) = (
            self.slot * self.settings.slot_minutes,
            self.scroll * self.settings.slot_minutes,
        );
        self.settings = cfg.settings;
        self.theme = cfg.theme;
        self.keymap = cfg.keymap;
        self.calendars = cfg.calendars;
        self.pending.clear();
        self.slot = self.clamp_slot(minutes / self.settings.slot_minutes);
        self.scroll = top / self.settings.slot_minutes;
        self.keep_slot_visible();
        self.invalidate();
    }

    /// Follows `path` from now on: any change to it is loaded and applied.
    pub fn watch_config(&mut self, path: PathBuf, database_path: Option<PathBuf>) {
        self.config_stamp = config::stamp(&path);
        self.config_path = Some(path);
        self.database_path = database_path;
    }

    fn check_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let stamp = config::stamp(&path);
        if stamp == self.config_stamp {
            return;
        }
        self.config_stamp = stamp;
        if self.config_stamp.is_some() {
            self.reload_config(&path);
        }
    }

    fn reload_config(&mut self, path: &std::path::Path) {
        match config::load(path) {
            Ok(cfg) => {
                let moved = cfg.database_path != self.database_path;
                self.apply_config(cfg);
                if moved {
                    self.info(
                        "config reloaded; database_path changes take effect when vcal restarts",
                    );
                } else {
                    self.info("config reloaded");
                }
            }
            Err(e) => {
                // the first link is the full path; the file name is enough here
                let name = path
                    .file_name()
                    .map_or("config".into(), |n| n.to_string_lossy());
                let why: Vec<String> = e.chain().skip(1).map(ToString::to_string).collect();
                self.error(format!(
                    "{name}: {}; keeping the previous settings",
                    why.join(": ")
                ));
            }
        }
    }

    /// Moves the cursor to `date`, at the current slot for today and the
    /// default hour otherwise.
    pub fn goto_date(&mut self, date: NaiveDate) {
        self.cursor = date;
        self.event_idx = 0;
        self.slot = if date == self.today() {
            self.slot_of(self.now)
        } else {
            self.settings.default_start_hour * 60 / self.settings.slot_minutes
        };
        self.slot = self.clamp_slot(self.slot);
        // a little context above the cursor
        self.scroll = self.slot.saturating_sub(4).max(self.settings.first_slot());
    }

    fn goto_today(&mut self) {
        self.goto_date(self.today());
    }

    pub fn today(&self) -> NaiveDate {
        time::local_date(self.now, self.tz)
    }

    pub fn point(&self) -> Point {
        Point {
            date: self.cursor,
            slot: self.slot,
        }
    }

    // --- time geometry ---------------------------------------------------

    fn clamp_slot(&self, slot: u32) -> u32 {
        let s = &self.settings;
        slot.clamp(s.first_slot(), s.end_slot().max(s.first_slot() + 1) - 1)
    }

    /// The instant a slot starts; slot == slots_per_day is next midnight.
    pub fn slot_ts(&self, date: NaiveDate, slot: u32) -> i64 {
        let minutes = slot * self.settings.slot_minutes;
        let day = date + Duration::days(i64::from(minutes / 1440));
        let t = NaiveTime::from_num_seconds_from_midnight_opt((minutes % 1440) * 60, 0)
            .unwrap_or(NaiveTime::MIN);
        time::resolve_local(day.and_time(t), self.tz)
    }

    /// The slot an instant falls in, on its own local day.
    pub fn slot_of(&self, ts: i64) -> u32 {
        let t = time::in_tz(ts, self.tz);
        (t.hour() * 60 + t.minute()) / self.settings.slot_minutes
    }

    /// The `[first, end)` slots an occurrence covers on `date`, if timed.
    pub fn slot_span(&self, o: &Occurrence, date: NaiveDate) -> Option<(u32, u32)> {
        if o.all_day {
            return None;
        }
        let per_day = self.settings.slots_per_day();
        let day_start = time::local_midnight(date, self.tz);
        let day_end = time::local_midnight(date.succ_opt()?, self.tz);
        let first = if o.start <= day_start {
            0
        } else {
            self.slot_of(o.start)
        };
        let end = if o.end >= day_end {
            per_day
        } else {
            let t = time::in_tz(o.end, self.tz);
            let minutes = t.hour() * 60 + t.minute();
            minutes.div_ceil(self.settings.slot_minutes)
        };
        Some((first, end.max(first + 1).min(per_day)))
    }

    pub fn week_start(&self, date: NaiveDate) -> NaiveDate {
        let ws = self.settings.week_start.num_days_from_monday();
        let off = (date.weekday().num_days_from_monday() + 7 - ws) % 7;
        date - Duration::days(i64::from(off))
    }

    /// Days the current view shows, `[first, end)`.
    pub fn visible_range(&self) -> (NaiveDate, NaiveDate) {
        match self.view {
            View::Month => {
                let first = self.week_start(self.cursor.with_day(1).unwrap_or(self.cursor));
                (first, first + Duration::days(42))
            }
            View::Week => {
                let first = self.week_start(self.cursor);
                (first, first + Duration::days(7))
            }
            View::Day => (self.cursor, self.cursor + Duration::days(1)),
            View::Agenda => {
                let first = self.week_start(self.cursor);
                (first, first + Duration::days(AGENDA_DAYS))
            }
        }
    }

    /// The agenda's entries in order: each `(day, index into day_events)`.
    pub fn agenda_entries(&self) -> Vec<(NaiveDate, usize)> {
        let (first, end) = self.visible_range();
        first
            .iter_days()
            .take_while(|d| *d < end)
            .flat_map(|d| (0..self.day_events(d).len()).map(move |i| (d, i)))
            .collect()
    }

    /// `j`/`k` in the agenda: to the next or previous event, on whatever
    /// day it is, looking up to a year past what's loaded.
    fn agenda_step(&mut self, down: bool) {
        let entries = self.agenda_entries();
        let here = (self.cursor, self.event_idx);
        let has_here = !self.day_events(self.cursor).is_empty();
        let found = if down {
            entries.iter().find(|e| {
                if has_here {
                    **e > here
                } else {
                    e.0 > self.cursor
                }
            })
        } else {
            entries.iter().rev().find(|e| {
                if has_here {
                    **e < here
                } else {
                    e.0 < self.cursor
                }
            })
        };
        if let Some(&(d, i)) = found {
            self.cursor = d;
            self.event_idx = i;
            return;
        }
        // past the loaded weeks: ask the database
        let (first, end) = self.visible_range();
        let (from, to) = if down {
            (end, end + Duration::days(366))
        } else {
            (first - Duration::days(366), first)
        };
        let tz = self.tz;
        let found = self
            .db
            .occurrences(
                time::local_midnight(from, tz),
                time::local_midnight(to, tz),
                tz,
                false,
            )
            .map(|list| {
                let mut hits = list.into_iter().filter(|o| self.passes_filter(o));
                if down { hits.next() } else { hits.next_back() }
            });
        match found {
            Ok(Some(o)) => {
                let day = if o.all_day {
                    time::floating_date(o.start)
                } else {
                    time::local_date(o.start, tz)
                };
                self.cursor = day;
                self.refresh();
                let list = self.day_events(day);
                self.event_idx = list.iter().position(|e| **e == o).unwrap_or(0);
            }
            Ok(None) => self.info(if down {
                "no later events within a year"
            } else {
                "no earlier events within a year"
            }),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    // --- data ------------------------------------------------------------

    fn read_stamp(&self) -> Option<String> {
        let path = self.db.path()?;
        std::fs::read_to_string(db::sentinel_path(path)).ok()
    }

    /// Reloads occurrences when the visible range changed or data did.
    pub fn refresh(&mut self) {
        let range = self.visible_range();
        if self.loaded == Some(range) {
            return;
        }
        let from = time::local_midnight(range.0, self.tz);
        let to = time::local_midnight(range.1, self.tz);
        match self.db.occurrences(from, to, self.tz, false) {
            Ok(occ) => {
                self.cache = occ;
                self.loaded = Some(range);
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn invalidate(&mut self) {
        self.loaded = None;
    }

    /// Re-reads the visible range from the database.
    pub fn reload(&mut self) {
        self.invalidate();
        self.refresh();
    }

    pub fn occurs_on(&self, o: &Occurrence, date: NaiveDate) -> bool {
        if o.all_day {
            let (s, e) = (time::floating_date(o.start), time::floating_date(o.end));
            return s <= date && date < e;
        }
        let ds = time::local_midnight(date, self.tz);
        let de = date
            .succ_opt()
            .map_or(i64::MAX, |d| time::local_midnight(d, self.tz));
        o.start < de && (o.end > ds || (o.start == o.end && o.start >= ds))
    }

    fn passes_filter(&self, o: &Occurrence) -> bool {
        self.filter.as_deref().is_none_or(|f| matches(o, f))
    }

    /// The day's occurrences (filtered by any search), all-day first.
    pub fn day_events(&self, date: NaiveDate) -> Vec<&Occurrence> {
        self.cache
            .iter()
            .filter(|o| self.occurs_on(o, date) && self.passes_filter(o))
            .collect()
    }

    /// The event commands act on: the highlighted entry of the day's list,
    /// or in week/day grids the timed event under the cursor slot.
    pub fn target(&self) -> Option<Occurrence> {
        let list = self.day_events(self.cursor);
        if self.focus == Focus::Events || self.view.by_days() {
            return list
                .get(self.event_idx.min(list.len().saturating_sub(1)))
                .map(|o| (*o).clone());
        }
        list.into_iter()
            .find(|o| {
                self.slot_span(o, self.cursor)
                    .is_some_and(|(a, b)| (a..b).contains(&self.slot))
            })
            .cloned()
    }

    pub fn is_target(&self, o: &Occurrence) -> bool {
        self.target()
            .is_some_and(|t| t.event_id == o.event_id && t.original_start == o.original_start)
    }

    /// `(low, high)` corners of the visual selection.
    pub fn selection(&self) -> Option<(Point, Point)> {
        let anchor = self.anchor?;
        let here = self.point();
        Some((anchor.min(here), anchor.max(here)))
    }

    pub fn in_selection(&self, p: Point) -> bool {
        let Some((lo, hi)) = self.selection() else {
            return false;
        };
        if self.view.by_days() {
            (lo.date..=hi.date).contains(&p.date)
        } else {
            lo <= p && p <= hi
        }
    }

    /// Occurrences that start inside the selection. Month view selects
    /// days (all-day events included); week/day views select time, so
    /// only timed events.
    fn selected_occurrences(&self) -> Result<Vec<Occurrence>> {
        let Some((lo, hi)) = self.selection() else {
            return Ok(vec![]);
        };
        let (from, to) = if self.view.by_days() {
            (
                time::local_midnight(lo.date, self.tz),
                time::local_midnight(hi.date + Duration::days(1), self.tz),
            )
        } else {
            (
                self.slot_ts(lo.date, lo.slot),
                self.slot_ts(hi.date, hi.slot + 1),
            )
        };
        let month = self.view.by_days();
        Ok(self
            .db
            .occurrences(from, to, self.tz, false)?
            .into_iter()
            .filter(|o| self.passes_filter(o))
            .filter(|o| {
                if o.all_day {
                    let d = time::floating_date(o.start);
                    month && lo.date <= d && d <= hi.date
                } else {
                    from <= o.start && o.start < to
                }
            })
            .collect())
    }

    // --- messages --------------------------------------------------------

    pub fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message {
            text: text.into(),
            error: true,
        });
    }

    /// Runs a write; reports its message or error and reloads.
    fn write(&mut self, f: impl FnOnce(&mut Db) -> Result<String>) -> bool {
        let result = f(&mut self.db);
        self.invalidate();
        self.stamp = self.read_stamp();
        match result {
            Ok(msg) => {
                self.info(msg);
                true
            }
            Err(e) => {
                self.error(format!("{e:#}"));
                false
            }
        }
    }

    fn states(&self, ids: &[String]) -> Result<States> {
        ids.iter()
            .map(|id| Ok((id.clone(), self.db.snapshot(id)?)))
            .collect()
    }

    /// [`App::write`], recorded for undo: `ids` are the events `f` may
    /// create, change or delete.
    fn record(&mut self, mut ids: Vec<String>, f: impl FnOnce(&mut Db) -> Result<String>) -> bool {
        ids.sort();
        ids.dedup();
        let before = match self.states(&ids) {
            Ok(s) => s,
            Err(e) => {
                self.error(format!("{e:#}"));
                return false;
            }
        };
        let ok = self.write(f);
        if ok {
            let label = self
                .message
                .as_ref()
                .map(|m| m.text.clone())
                .unwrap_or_default();
            self.push_change(label, before, &ids);
        }
        ok
    }

    /// Records a change already made: the events in `ids` went from `before`
    /// to how they are now.
    fn push_change(&mut self, label: String, before: States, ids: &[String]) {
        match self.states(ids) {
            Ok(after) if after != before => {
                self.undo.push(Change {
                    label,
                    before,
                    after,
                });
                if self.undo.len() > UNDO_DEPTH {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            Ok(_) => {}
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// Reverts a change if its events are still as it left them, and
    /// returns the change that reverts this one.
    fn revert(&mut self, c: Change, verb: &str) -> Option<Change> {
        let ids: Vec<String> = c.after.iter().map(|(id, _)| id.clone()).collect();
        match self.states(&ids) {
            Ok(now) if now == c.after => {}
            Ok(_) => {
                self.error(format!(
                    "can't {verb} \"{}\": changed since (another window or a sync)",
                    c.label
                ));
                return None;
            }
            Err(e) => {
                self.error(format!("{e:#}"));
                return None;
            }
        }
        let label = c.label.clone();
        if !self.write(|db| db.restore(&c.before).map(|()| format!("{verb}: {label}"))) {
            return None;
        }
        // follow the change: the first event that exists afterwards
        if let Some(ev) = c
            .before
            .iter()
            .find_map(|(_, s)| s.as_ref().map(|s| &s.event))
        {
            self.cursor = if ev.all_day {
                time::floating_date(ev.start)
            } else {
                time::local_date(ev.start, self.tz)
            };
            if !ev.all_day {
                self.slot = self.clamp_slot(self.slot_of(ev.start));
            }
        }
        let now = self.states(&ids).ok()?;
        Some(Change {
            label: c.label,
            before: c.after,
            after: now,
        })
    }

    fn undo(&mut self) {
        let Some(c) = self.undo.pop() else {
            return self.info("nothing to undo");
        };
        if let Some(inverse) = self.revert(c, "undid") {
            self.redo.push(inverse);
        }
    }

    fn redo(&mut self) {
        let Some(c) = self.redo.pop() else {
            return self.info("nothing to redo");
        };
        if let Some(inverse) = self.revert(c, "redid") {
            self.undo.push(inverse);
        }
    }

    // --- update ----------------------------------------------------------

    /// Periodic housekeeping: clock, and reloading after writes from
    /// other processes.
    pub fn tick(&mut self, now: i64) {
        self.check_config();
        let day_changed = time::local_date(now, self.tz) != self.today();
        self.now = now;
        let stamp = self.read_stamp();
        if stamp != self.stamp || day_changed {
            self.stamp = stamp;
            self.invalidate();
        }
        self.refresh();
    }

    pub fn on_key_event(&mut self, ev: KeyEvent) {
        self.on_key(Key::from(ev));
    }

    pub fn on_key(&mut self, key: Key) {
        match self.mode {
            Mode::Insert => self.form_key(key),
            Mode::Command | Mode::Search => self.line_key(key),
            Mode::Prompt => self.prompt_key(key),
            Mode::Help => self.mode = Mode::Normal,
            Mode::Normal | Mode::Visual => self.chord(key),
        }
        self.refresh();
        let n = self.day_events(self.cursor).len();
        self.event_idx = self.event_idx.min(n.saturating_sub(1));
        if self.focus == Focus::Events && n == 0 {
            self.focus = Focus::Grid;
        }
        self.keep_slot_visible();
    }

    pub fn keep_slot_visible(&mut self) {
        let s = &self.settings;
        let rows = self.viewport_slots.max(1);
        if self.slot < self.scroll {
            self.scroll = self.slot;
        } else if self.slot >= self.scroll + rows {
            self.scroll = self.slot + 1 - rows;
        }
        let max_scroll = s.end_slot().saturating_sub(rows).max(s.first_slot());
        self.scroll = self.scroll.clamp(s.first_slot(), max_scroll);
    }

    fn chord(&mut self, key: Key) {
        if self.pending.is_empty() {
            if self.count.is_none() {
                self.message = None;
            }
            let digit = match key.code {
                KeyCode::Char(c) if key.mods.is_empty() => c.to_digit(10),
                _ => None,
            };
            if let Some(d) = digit.filter(|_| keymap::starts_count(key) || self.count.is_some()) {
                self.count = Some((self.count.unwrap_or(0) * 10 + d).min(MAX_COUNT));
                return;
            }
        }
        self.pending.push(key);
        let mode = if self.mode == Mode::Visual {
            KeyMode::Visual
        } else {
            KeyMode::Normal
        };
        match self.keymap.resolve(mode, &self.pending) {
            Resolved::Action(action) => {
                self.pending.clear();
                let count = self.count.take().unwrap_or(1);
                self.perform_counted(action, count);
            }
            Resolved::Pending => {}
            Resolved::Unbound => {
                self.pending.clear();
                self.count = None;
            }
        }
    }

    /// Runs an action `count` times where that means something (motions,
    /// `n`/`N`, `>`/`<`); other actions ignore the count.
    fn perform_counted(&mut self, action: Action, count: u32) {
        let n = i64::from(count);
        match action {
            Action::Left => self.move_days(-n),
            Action::Right => self.move_days(n),
            Action::WeekNext => self.move_days(7 * n),
            Action::WeekPrev => self.move_days(-7 * n),
            // in one step, so 3} from Jan 31 lands on Apr 30, not Apr 28
            Action::MonthNext => self.move_months(count as i32),
            Action::MonthPrev => self.move_months(-(count as i32)),
            Action::MoveLater => self.nudge(false, n),
            Action::MoveEarlier => self.nudge(false, -n),
            Action::Lengthen => self.nudge(true, n),
            Action::Shorten => self.nudge(true, -n),
            Action::ShiftLater | Action::ShiftEarlier => {
                self.shift(action == Action::ShiftLater, n)
            }
            Action::Down
            | Action::Up
            | Action::SearchNext
            | Action::SearchPrev
            | Action::Undo
            | Action::Redo => {
                for _ in 0..count {
                    self.perform(action);
                    if self.message.as_ref().is_some_and(|m| m.error) {
                        break;
                    }
                }
            }
            _ => self.perform(action),
        }
    }

    fn move_days(&mut self, n: i64) {
        self.cursor += Duration::days(n);
        self.event_idx = 0;
    }

    fn move_months(&mut self, n: i32) {
        let months = Months::new(n.unsigned_abs());
        let moved = if n >= 0 {
            self.cursor.checked_add_months(months)
        } else {
            self.cursor.checked_sub_months(months)
        };
        self.cursor = moved.unwrap_or(self.cursor);
        self.event_idx = 0;
    }

    fn move_vertical(&mut self, down: bool) {
        if self.focus == Focus::Events {
            let n = self.day_events(self.cursor).len();
            self.event_idx = if down {
                (self.event_idx + 1).min(n.saturating_sub(1))
            } else {
                self.event_idx.saturating_sub(1)
            };
            return;
        }
        match self.view {
            View::Month => self.move_days(if down { 7 } else { -7 }),
            View::Agenda => self.agenda_step(down),
            View::Week | View::Day => {
                let slot = if down {
                    self.slot + 1
                } else {
                    self.slot.saturating_sub(1)
                };
                self.slot = self.clamp_slot(slot);
            }
        }
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.anchor = None;
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
        }
    }

    pub fn perform(&mut self, action: Action) {
        let visual = self.mode == Mode::Visual;
        match action {
            Action::Left => self.move_days(-1),
            Action::Right => self.move_days(1),
            Action::Down => self.move_vertical(true),
            Action::Up => self.move_vertical(false),
            Action::WeekNext => self.move_days(7),
            Action::WeekPrev => self.move_days(-7),
            Action::MonthNext => self.move_months(1),
            Action::MonthPrev => self.move_months(-1),
            Action::RowStart => self.cursor = self.week_start(self.cursor),
            Action::RowEnd => self.cursor = self.week_start(self.cursor) + Duration::days(6),
            Action::ViewportFirst | Action::ViewportLast => {
                let last = action == Action::ViewportLast;
                if self.focus == Focus::Events {
                    let n = self.day_events(self.cursor).len();
                    self.event_idx = if last { n.saturating_sub(1) } else { 0 };
                    return;
                }
                match self.view {
                    View::Month => {
                        let first = self.cursor.with_day(1).unwrap_or(self.cursor);
                        self.cursor = if last {
                            first
                                .checked_add_months(Months::new(1))
                                .map_or(first, |d| d - Duration::days(1))
                        } else {
                            first
                        };
                    }
                    View::Week => self.perform(if last {
                        Action::RowEnd
                    } else {
                        Action::RowStart
                    }),
                    View::Day => {
                        self.slot = if last {
                            self.clamp_slot(self.settings.end_slot())
                        } else {
                            self.clamp_slot(0)
                        }
                    }
                    // the first or last event loaded
                    View::Agenda => {
                        let entries = self.agenda_entries();
                        let pick = if last {
                            entries.last()
                        } else {
                            entries.first()
                        };
                        if let Some(&(d, i)) = pick {
                            self.cursor = d;
                            self.event_idx = i;
                        }
                        return;
                    }
                }
                self.event_idx = 0;
            }
            Action::Today => self.goto_today(),
            Action::ViewNext | Action::ViewPrev => {
                let i = View::ALL.iter().position(|v| *v == self.view).unwrap_or(0);
                let n = View::ALL.len();
                let step = if action == Action::ViewNext { 1 } else { n - 1 };
                self.set_view(View::ALL[(i + step) % n]);
            }
            Action::ViewMonth => self.set_view(View::Month),
            Action::ViewWeek => self.set_view(View::Week),
            Action::ViewDay => self.set_view(View::Day),
            Action::ViewAgenda => self.set_view(View::Agenda),
            Action::Focus if self.view == View::Agenda => self.perform(Action::Change),
            Action::Focus => {
                if self.focus == Focus::Events {
                    self.perform(Action::Change);
                } else if !self.day_events(self.cursor).is_empty() {
                    self.focus = Focus::Events;
                    if let Some(i) = self
                        .target()
                        .and_then(|t| self.day_events(self.cursor).iter().position(|o| **o == t))
                    {
                        self.event_idx = i;
                    }
                }
            }
            Action::NewEvent => self.open_new(),
            Action::NewAllDay => {
                let form = Form::new_all_day(self.cursor, self.cursor);
                self.show_form(form);
            }
            Action::Delete if visual => match self.selected_occurrences() {
                Ok(list) if list.is_empty() => {
                    self.exit_visual();
                    self.info("nothing selected");
                }
                Ok(list) => {
                    self.exit_visual();
                    if list.iter().any(Occurrence::recurring) {
                        self.ask(Prompt::SelectionScope(list));
                    } else {
                        self.ask(Prompt::Delete(list));
                    }
                }
                Err(e) => self.error(format!("{e:#}")),
            },
            Action::Delete | Action::Change => {
                let Some(occ) = self.target() else {
                    self.info("no event here");
                    return;
                };
                let then = if action == Action::Delete {
                    ScopeAction::Delete
                } else {
                    ScopeAction::Change
                };
                if occ.recurring() {
                    self.ask(Prompt::Scope { occ, then });
                } else if then == ScopeAction::Delete {
                    self.ask(Prompt::Delete(vec![occ]));
                } else {
                    self.open_edit(&occ, true);
                }
            }
            Action::Yank if visual => match self.selected_occurrences() {
                Ok(list) => {
                    self.exit_visual();
                    self.info(format!("yanked {} event(s)", list.len()));
                    self.yanked = list;
                }
                Err(e) => self.error(format!("{e:#}")),
            },
            Action::Yank => match self.target() {
                Some(occ) => {
                    self.info(format!("yanked \"{}\"", occ.title));
                    self.yanked = vec![occ];
                }
                None => self.info("no event here"),
            },
            Action::Paste => self.paste(),
            Action::MoveLater => self.nudge(false, 1),
            Action::MoveEarlier => self.nudge(false, -1),
            Action::Lengthen => self.nudge(true, 1),
            Action::Shorten => self.nudge(true, -1),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::ShiftLater | Action::ShiftEarlier => {
                self.shift(action == Action::ShiftLater, 1)
            }
            Action::Search => {
                self.mode = Mode::Search;
                self.line = TextInput::default();
                self.filter = None;
            }
            Action::SearchNext | Action::SearchPrev => {
                self.search_step(action == Action::SearchNext)
            }
            Action::CommandLine => {
                self.mode = Mode::Command;
                self.line = TextInput::default();
            }
            Action::Visual => {
                self.anchor = Some(self.point());
                self.mode = Mode::Visual;
                self.focus = Focus::Grid;
            }
            Action::Escape => {
                if visual {
                    self.exit_visual();
                } else if self.focus == Focus::Events {
                    self.focus = Focus::Grid;
                } else if self.filter.is_some() {
                    self.filter = None;
                    self.invalidate();
                }
                self.message = None;
            }
            Action::Help => self.mode = Mode::Help,
            Action::Quit => self.quit = true,
        }
    }

    fn exit_visual(&mut self) {
        self.anchor = None;
        self.mode = Mode::Normal;
    }

    fn ask(&mut self, prompt: Prompt) {
        self.prompt = Some(prompt);
        self.mode = Mode::Prompt;
    }

    // --- editing ---------------------------------------------------------

    fn open_new(&mut self) {
        let dur = self.settings.default_duration;
        let form = match (self.selection(), self.view.by_days()) {
            (Some((lo, hi)), true) => Form::new_all_day(lo.date, hi.date),
            (Some((lo, hi)), _) => Form::new_timed(
                self.slot_ts(lo.date, lo.slot),
                self.slot_ts(hi.date, hi.slot + 1),
                self.tz,
            ),
            (None, true) => {
                let start = if self.cursor == self.today() {
                    // the next slot boundary from now
                    self.slot_ts(self.cursor, self.slot_of(self.now) + 1)
                } else {
                    self.slot_ts(
                        self.cursor,
                        self.settings.default_start_hour * 60 / self.settings.slot_minutes,
                    )
                };
                Form::new_timed(start, start + dur, self.tz)
            }
            (None, _) => {
                let start = self.slot_ts(self.cursor, self.slot);
                Form::new_timed(start, start + dur, self.tz)
            }
        };
        self.exit_visual();
        self.show_form(form);
    }

    /// Opens the dialog, offering the Calendar field when calendars are
    /// configured and filling in the default one for a new event.
    fn show_form(&mut self, mut form: Form) {
        form.show_calendar = !self.calendars.is_empty();
        if matches!(form.target, super::form::Target::New) {
            form.calendar = TextInput::new(self.default_calendar().unwrap_or_default());
        }
        self.form = Some(form);
        self.mode = Mode::Insert;
    }

    pub fn default_calendar(&self) -> Option<String> {
        self.calendars
            .iter()
            .find(|c| c.default)
            .map(|c| c.name.clone())
    }

    fn open_edit(&mut self, occ: &Occurrence, series: bool) {
        match self.db.get(&occ.event_id) {
            Ok(Some(ev)) => {
                let form = if series {
                    Form::edit_series(ev, self.tz)
                } else {
                    Form::edit_occurrence(ev, occ, self.tz)
                };
                self.show_form(form);
            }
            Ok(None) => self.error("that event no longer exists"),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn form_key(&mut self, key: Key) {
        let Some(form) = self.form.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        if key.code == KeyCode::Esc {
            self.form = None;
            self.mode = Mode::Normal;
            self.info("discarded");
            return;
        }
        if !form.handle(key, self.tz, &self.settings) {
            return;
        }
        let submit = match form.submit(self.tz, &self.settings, self.now) {
            Ok(s) => s,
            Err(e) => {
                form.error = Some(format!("{e:#}"));
                return;
            }
        };
        if let Submit::Insert(e) | Submit::Update(e) = &submit
            && let Some(name) = &e.calendar
        {
            let unchanged = matches!(&form.target, super::form::Target::Series(old) if old.calendar.as_ref() == Some(name));
            if !unchanged && let Err(err) = config::writable_calendar(&self.calendars, name) {
                form.error = Some(format!("{err:#}"));
                return;
            }
        }
        let (start, all_day) = match &submit {
            Submit::Insert(e) | Submit::Update(e) => (e.start, e.all_day),
            Submit::Exception(x) => (x.start.unwrap_or(x.original_start), form.all_day),
        };
        let id = match &submit {
            Submit::Insert(e) | Submit::Update(e) => e.id.clone(),
            Submit::Exception(x) => x.event_id.clone(),
        };
        let before = self.states(std::slice::from_ref(&id)).ok();
        let result = match submit {
            Submit::Insert(ev) => self
                .db
                .insert(ev)
                .map(|e| format!("created \"{}\"", e.title)),
            Submit::Update(ev) => self.db.update(ev).map(|e| format!("saved \"{}\"", e.title)),
            Submit::Exception(x) => self
                .db
                .set_exception(x)
                .map(|()| "saved this occurrence".into()),
        };
        self.invalidate();
        self.stamp = self.read_stamp();
        match result {
            Ok(msg) => {
                self.form = None;
                self.mode = Mode::Normal;
                if let Some(before) = before {
                    self.push_change(msg.clone(), before, &[id]);
                }
                self.info(msg);
                self.cursor = if all_day {
                    time::floating_date(start)
                } else {
                    time::local_date(start, self.tz)
                };
                if !all_day {
                    self.slot = self.clamp_slot(self.slot_of(start));
                }
            }
            // keep the dialog open so nothing typed is lost
            Err(e) => {
                if let Some(form) = self.form.as_mut() {
                    form.error = Some(format!("{e:#}"));
                }
            }
        }
    }

    fn prompt_key(&mut self, key: Key) {
        let Some(prompt) = self.prompt.take() else {
            self.mode = Mode::Normal;
            return;
        };
        self.mode = Mode::Normal;
        let KeyCode::Char(c) = key.code else {
            if key.code == KeyCode::Enter && matches!(prompt, Prompt::Delete(_)) {
                self.delete(prompt);
            } else if key.code != KeyCode::Esc {
                self.ask(prompt);
            }
            return;
        };
        match (&prompt, c.to_ascii_lowercase()) {
            (Prompt::Delete(_), 'y') => self.delete(prompt),
            (Prompt::Delete(_), 'n') => {}
            (Prompt::Scope { occ, then }, 'o' | 'a' | 's') => {
                let series = c != 'o';
                match then {
                    ScopeAction::Change => self.open_edit(&occ.clone(), series),
                    ScopeAction::Delete if series => {
                        let mut whole = occ.clone();
                        whole.original_start = None;
                        self.ask(Prompt::Delete(vec![whole]));
                    }
                    ScopeAction::Delete => self.ask(Prompt::Delete(vec![occ.clone()])),
                }
            }
            (Prompt::SelectionScope(list), 'o') => self.ask(Prompt::Delete(list.clone())),
            (Prompt::SelectionScope(list), 'a' | 's') => {
                // one whole-event entry per event, in selection order
                let mut seen = std::collections::HashSet::new();
                let whole = list
                    .iter()
                    .filter(|o| seen.insert(o.event_id.clone()))
                    .map(|o| Occurrence {
                        original_start: None,
                        ..o.clone()
                    })
                    .collect();
                self.ask(Prompt::Delete(whole));
            }
            _ => self.ask(prompt),
        }
    }

    fn delete(&mut self, prompt: Prompt) {
        let Prompt::Delete(list) = prompt else { return };
        let ids = list.iter().map(|o| o.event_id.clone()).collect();
        self.record(ids, |db| {
            let mut seen = std::collections::HashSet::new();
            for o in &list {
                match o.original_start {
                    Some(at) => db.cancel_occurrence(&o.event_id, at)?,
                    None if seen.insert(o.event_id.clone()) => {
                        db.delete(&o.event_id)?;
                    }
                    None => {}
                }
            }
            Ok(match list.as_slice() {
                [one] => format!("deleted \"{}\"", one.title),
                many => format!("deleted {} events", many.len()),
            })
        });
    }

    fn paste(&mut self) {
        let Some(base) = self.yanked.first().cloned() else {
            self.info("nothing yanked");
            return;
        };
        let tz = self.tz;
        let base_date = if base.all_day {
            time::floating_date(base.start)
        } else {
            time::local_date(base.start, tz)
        };
        let days = (self.cursor - base_date).num_days();
        // Week/day views paste at the cursor slot; month view keeps the time of day.
        let instant_shift = (!self.view.by_days() && !base.all_day)
            .then(|| self.slot_ts(self.cursor, self.slot) - base.start);
        let default_calendar = self.default_calendar();
        let copies: Vec<Event> = self
            .yanked
            .iter()
            .map(|o| {
                let start = if o.all_day {
                    o.start + days * DAY
                } else if let Some(shift) = instant_shift {
                    o.start + shift
                } else {
                    let local = time::in_tz(o.start, tz).naive_local() + Duration::days(days);
                    time::resolve_local(local, tz)
                };
                let mut ev = Event::new(o.title.clone(), start, start + (o.end - o.start));
                ev.description = o.description.clone();
                ev.category = o.category.clone();
                ev.location = o.location.clone();
                ev.reminder = o.reminder;
                ev.all_day = o.all_day;
                // a copy is yours, wherever the original came from
                ev.calendar = default_calendar.clone();
                if !o.all_day {
                    ev.tzid = Some(tz.name().to_string());
                }
                ev
            })
            .collect();
        let ids = copies.iter().map(|e| e.id.clone()).collect();
        self.record(ids, |db| {
            let n = copies.len();
            for ev in copies {
                db.insert(ev)?;
            }
            Ok(format!("pasted {n} event(s)"))
        });
    }

    fn point_add(&self, p: Point, slots: i64) -> Point {
        let per_day = i64::from(self.settings.slots_per_day());
        let total = i64::from(p.slot) + slots;
        Point {
            date: p.date + Duration::days(total.div_euclid(per_day)),
            slot: total.rem_euclid(per_day) as u32,
        }
    }

    /// `>` / `<` in visual mode: move the selected events (and the
    /// selection with them) by `steps` days in month view, slots otherwise.
    fn shift(&mut self, later: bool, steps: i64) {
        let list = match self.selected_occurrences() {
            Ok(list) => list,
            Err(e) => return self.error(format!("{e:#}")),
        };
        let sign = if later { steps } else { -steps };
        let month = self.view.by_days();
        let delta = sign
            * if month {
                DAY
            } else {
                i64::from(self.settings.slot_minutes) * 60
            };
        let ids = list.iter().map(|o| o.event_id.clone()).collect();
        let moved = self.record(ids, |db| {
            for o in &list {
                set_span(db, o, o.start + delta, o.end + delta)?;
            }
            Ok(format!(
                "moved {} event(s) {}",
                list.len(),
                if later { "later" } else { "earlier" }
            ))
        });
        if moved {
            if month {
                let days = Duration::days(sign);
                self.anchor = self.anchor.map(|p| Point {
                    date: p.date + days,
                    ..p
                });
                self.cursor += days;
            } else {
                self.anchor = self.anchor.map(|p| self.point_add(p, sign));
                let here = self.point_add(self.point(), sign);
                self.cursor = here.date;
                self.slot = here.slot;
            }
        }
    }

    /// `J`/`K` (move) and `>`/`<` (resize) in normal mode, on the event the
    /// cursor is on, by `steps` slots, or days for all-day events and for
    /// moves in the month and agenda views. A repeating event changes only
    /// this occurrence.
    fn nudge(&mut self, resize: bool, steps: i64) {
        let Some(o) = self.target() else {
            return self.info("no event here");
        };
        let slot = i64::from(self.settings.slot_minutes) * 60;
        let tz = self.tz;
        let (start, end) = if resize {
            let unit = if o.all_day { DAY } else { slot };
            let end = o.end + steps * unit;
            if end - o.start < unit {
                return self.error(format!(
                    "\"{}\" can't get shorter than {}",
                    o.title,
                    if o.all_day {
                        "a day".to_string()
                    } else {
                        time::format_minutes(unit / 60)
                    }
                ));
            }
            (o.start, end)
        } else if o.all_day {
            (o.start + steps * DAY, o.end + steps * DAY)
        } else if self.view.by_days() {
            // whole days keep the time of day across DST changes
            let shift = |ts: i64| {
                time::resolve_local(
                    time::in_tz(ts, tz).naive_local() + Duration::days(steps),
                    tz,
                )
            };
            let start = shift(o.start);
            (start, start + (o.end - o.start))
        } else {
            (o.start + steps * slot, o.end + steps * slot)
        };
        let what = if o.recurring() {
            "this occurrence of "
        } else {
            ""
        };
        let verb = match (resize, steps > 0) {
            (true, true) => "lengthened",
            (true, false) => "shortened",
            (false, true) => "moved later",
            (false, false) => "moved earlier",
        };
        let label = format!("{verb} {what}\"{}\"", o.title);
        let target = o.clone();
        if !self.record(vec![o.event_id.clone()], |db| {
            set_span(db, &target, start, end).map(|()| label)
        }) {
            return;
        }
        // follow it, so another J keeps moving the same event
        if !resize {
            self.cursor = if o.all_day {
                time::floating_date(start)
            } else {
                time::local_date(start, tz)
            };
            if !o.all_day && !self.view.by_days() {
                self.slot = self.clamp_slot(self.slot_of(start));
            }
        }
        self.refresh();
        if let Some(i) = self
            .day_events(self.cursor)
            .iter()
            .position(|e| e.event_id == o.event_id && e.original_start == o.original_start)
        {
            self.event_idx = i;
        }
    }

    fn search_step(&mut self, forward: bool) {
        let Some(needle) = self.filter.clone() else {
            self.info("no search; use / first");
            return;
        };
        let tz = self.tz;
        let (from, to) = if forward {
            let start = self.cursor + Duration::days(1);
            (start, start + Duration::days(366))
        } else {
            (self.cursor - Duration::days(366), self.cursor)
        };
        let found = self
            .db
            .occurrences(
                time::local_midnight(from, tz),
                time::local_midnight(to, tz),
                tz,
                false,
            )
            .map(|list| {
                let mut hits = list.into_iter().filter(|o| matches(o, &needle));
                if forward {
                    hits.next()
                } else {
                    hits.next_back()
                }
            });
        match found {
            Ok(Some(o)) => {
                self.cursor = if o.all_day {
                    time::floating_date(o.start)
                } else {
                    time::local_date(o.start, tz)
                };
                if !o.all_day {
                    self.slot = self.clamp_slot(self.slot_of(o.start));
                }
                self.refresh();
                self.event_idx = self
                    .day_events(self.cursor)
                    .iter()
                    .position(|e| **e == o)
                    .unwrap_or(0);
                self.info(format!("/{needle}: {}", o.title));
            }
            Ok(None) => self.error(format!("no match for \"{needle}\" within a year")),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn line_key(&mut self, key: Key) {
        let search = self.mode == Mode::Search;
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                if search {
                    self.filter = None;
                }
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let text = self.line.text.trim().to_string();
                if search {
                    self.filter = Some(text).filter(|t| !t.is_empty());
                    if self.filter.is_some() && self.day_events(self.cursor).is_empty() {
                        self.search_step(true);
                    }
                } else {
                    self.command(&text);
                }
            }
            KeyCode::Backspace if self.line.text.is_empty() => {
                self.mode = Mode::Normal;
                if search {
                    self.filter = None;
                }
            }
            _ => {
                self.line.handle(key);
                if search {
                    self.filter = Some(self.line.text.clone()).filter(|t| !t.is_empty());
                }
            }
        }
    }

    /// Ex commands.
    pub fn command(&mut self, input: &str) {
        let (cmd, arg) = match input.split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (input, ""),
        };
        match cmd {
            "" => {}
            "q" | "q!" | "quit" | "qa" | "qa!" | "wq" | "wq!" | "x" | "xa" => self.quit = true,
            "w" | "write" | "export" if !arg.is_empty() => self.export(arg),
            "w" | "write" => self.info("everything is saved as you go; :w <file.ics> exports"),
            "j" | "jump" | "goto" | "e" if !arg.is_empty() => self.jump(arg),
            "today" => self.goto_today(),
            "view" => match View::parse(arg) {
                Some(v) => self.set_view(v),
                None => self.error("usage: :view month|week|day|agenda"),
            },
            "month" | "week" | "day" | "agenda" => {
                self.set_view(View::parse(cmd).unwrap_or_default())
            }
            "import" if !arg.is_empty() => self.import(arg),
            "sync" => self.sync(arg),
            "a" | "add" if !arg.is_empty() => self.quick_add(arg),
            "a" | "add" => self.error("usage: :add Dentist tomorrow 14:00 45m @health"),
            "noh" | "nohlsearch" => self.filter = None,
            "h" | "help" => self.mode = Mode::Help,
            "reload" => {
                self.reload();
                self.info("reloaded");
                if let Some(path) = self.config_path.clone() {
                    self.config_stamp = config::stamp(&path);
                    self.reload_config(&path);
                }
            }
            _ if time::parse_when(input, self.tz, self.now).is_ok() => self.jump(input),
            _ => self.error(format!("not a command: {input}")),
        }
    }

    fn jump(&mut self, arg: &str) {
        match time::parse_when(arg, self.tz, self.now) {
            Ok(when) => {
                self.goto_date(when.date(self.tz));
                if let time::When::Instant(ts) = when {
                    self.slot = self.clamp_slot(self.slot_of(ts));
                }
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// `:sync [names]`, as `vcal sync`. It waits on the network, so the
    /// screen holds still until it's done.
    fn sync(&mut self, arg: &str) {
        if self.calendars.is_empty() {
            return self.error("no [calendars] configured (see vcal config default)");
        }
        let cfg = Config {
            calendars: self.calendars.clone(),
            ..Config::default()
        };
        let names: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
        let result = crate::sync::sync(&mut self.db, &cfg, &names, self.tz);
        self.invalidate();
        self.stamp = self.read_stamp();
        match result {
            Ok(outcomes) => {
                let failed: Vec<String> = outcomes
                    .iter()
                    .filter(|o| o.failed)
                    .map(|o| format!("{}: {}", o.name, o.summary))
                    .collect();
                let warnings: usize = outcomes.iter().map(|o| o.warnings.len()).sum();
                if !failed.is_empty() {
                    self.error(failed.join("; "));
                } else {
                    let mut msg: Vec<String> = outcomes
                        .iter()
                        .map(|o| format!("{}: {}", o.name, o.summary))
                        .collect();
                    if warnings > 0 {
                        msg.push(format!("{warnings} warnings (vcal sync shows them)"));
                    }
                    self.info(msg.join(" · "));
                }
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    /// `:add <words>`: an event from one line (see `crate::quick`), on the
    /// cursor's day unless the words name one.
    fn quick_add(&mut self, arg: &str) {
        let ev = match crate::quick::parse(
            arg,
            self.cursor,
            self.today(),
            self.tz,
            self.settings.default_duration,
        ) {
            Ok(ev) => ev,
            Err(e) => return self.error(format!("{e:#}")),
        };
        let mut ev = ev;
        ev.calendar = self.default_calendar();
        let (start, all_day) = (ev.start, ev.all_day);
        let summary = crate::quick::describe(&ev, self.tz, self.settings.clock_24h);
        let id = ev.id.clone();
        if self.record(vec![id], |db| {
            db.insert(ev).map(|_| format!("created {summary}"))
        }) {
            self.cursor = if all_day {
                time::floating_date(start)
            } else {
                time::local_date(start, self.tz)
            };
            if !all_day {
                self.slot = self.clamp_slot(self.slot_of(start));
            }
            self.event_idx = 0;
        }
    }

    fn export(&mut self, arg: &str) {
        let path = expand_home(arg);
        let result = self.db.events().and_then(|events| {
            let n = events.len();
            let text = crate::ics::write::calendar(&self.db.with_exceptions(events)?, self.now);
            std::fs::write(&path, text)
                .with_context(|| format!("can't write {}", path.display()))?;
            Ok(n)
        });
        match result {
            Ok(n) => self.info(format!("exported {n} events to {}", path.display())),
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn import(&mut self, arg: &str) {
        let path = expand_home(arg);
        let parsed = std::fs::read_to_string(&path)
            .with_context(|| format!("can't read {}", path.display()))
            .and_then(|text| crate::ics::parse(&text, self.tz));
        let parsed = match parsed {
            Ok(p) => p,
            Err(e) => return self.error(format!("{e:#}")),
        };
        let ids = parsed.events.iter().map(|e| e.id.clone()).collect();
        let warnings = parsed.warnings.len();
        self.record(ids, |db| {
            let (e, x) = db.import(parsed.events, parsed.exceptions)?;
            let mut msg = format!("imported {e} events, {x} exceptions");
            if warnings > 0 {
                msg.push_str(&format!(" ({warnings} warnings; vcal import shows them)"));
            }
            Ok(msg)
        });
    }

    /// Whether an occurrence starts within the next hour (drawn as urgent).
    pub fn is_soon(&self, o: &Occurrence) -> bool {
        !o.all_day && o.start > self.now && o.start - self.now <= 3600
    }

    pub fn is_past(&self, o: &Occurrence) -> bool {
        if o.all_day {
            time::floating_date(o.end) <= self.today()
        } else {
            o.end <= self.now
        }
    }

    pub fn is_cancelled(o: &Occurrence) -> bool {
        o.status == Status::Cancelled
    }
}
