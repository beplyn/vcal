//! The floating create/edit dialog: single-line inputs plus submission into
//! an insert, an update, or an occurrence exception.

use anyhow::{Result, bail, ensure};
use chrono::NaiveDate;
use chrono_tz::Tz;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::keymap::Key;
use crate::model::{Event, Exception, Occurrence};
use crate::settings::Settings;
use crate::time::{self, DAY, When};

/// A single-line editor with a char-indexed cursor and readline-ish keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.chars().count();
        TextInput { text, cursor }
    }

    fn byte(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Applies an editing key; false if the key isn't an editing key.
    pub fn handle(&mut self, key: Key) -> bool {
        let ctrl = key.mods.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char('b') if ctrl => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('f') if ctrl => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Char('u') if ctrl => {
                let at = self.byte(self.cursor);
                self.text.replace_range(..at, "");
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                let at = self.byte(self.cursor);
                self.text.truncate(at);
            }
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.text.chars().collect();
                let mut start = self.cursor;
                while start > 0 && chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                while start > 0 && !chars[start - 1].is_whitespace() {
                    start -= 1;
                }
                let (a, b) = (self.byte(start), self.byte(self.cursor));
                self.text.replace_range(a..b, "");
                self.cursor = start;
            }
            KeyCode::Char(c) if !ctrl && !key.mods.contains(KeyModifiers::ALT) => {
                let at = self.byte(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    let at = self.byte(self.cursor);
                    self.text.remove(at);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.len() {
                    let at = self.byte(self.cursor);
                    self.text.remove(at);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Start,
    End,
    AllDay,
    Category,
    Location,
    Reminder,
    Repeat,
    Notes,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Start => "Start",
            Field::End => "End",
            Field::AllDay => "All day",
            Field::Category => "Category",
            Field::Location => "Location",
            Field::Reminder => "Reminder",
            Field::Repeat => "Repeat",
            Field::Notes => "Notes",
        }
    }
}

const FIELDS: [Field; 9] = [
    Field::Title,
    Field::Start,
    Field::End,
    Field::AllDay,
    Field::Category,
    Field::Location,
    Field::Reminder,
    Field::Repeat,
    Field::Notes,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    New,
    /// The whole event (or series).
    Series(Event),
    /// One occurrence of a series, saved as an exception.
    Occurrence {
        event: Event,
        original_start: i64,
    },
}

/// What a successful submit asks the app to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Submit {
    Insert(Event),
    Update(Event),
    Exception(Exception),
}

#[derive(Debug, Clone)]
pub struct Form {
    pub target: Target,
    pub title: TextInput,
    pub start: TextInput,
    pub end: TextInput,
    pub all_day: bool,
    pub category: TextInput,
    pub location: TextInput,
    pub reminder: TextInput,
    pub repeat: TextInput,
    pub notes: TextInput,
    pub focus: Field,
    pub error: Option<String>,
}

fn fmt_instant(ts: i64, tz: Tz) -> String {
    time::in_tz(ts, tz).format("%Y-%m-%d %H:%M").to_string()
}

/// The inclusive last day of an all-day span stored with an exclusive end.
fn last_day(start: i64, end: i64) -> NaiveDate {
    time::floating_date(end.max(start + DAY) - DAY)
}

impl Form {
    fn blank(target: Target) -> Self {
        Form {
            target,
            title: TextInput::default(),
            start: TextInput::default(),
            end: TextInput::default(),
            all_day: false,
            category: TextInput::default(),
            location: TextInput::default(),
            reminder: TextInput::default(),
            repeat: TextInput::default(),
            notes: TextInput::default(),
            focus: Field::Title,
            error: None,
        }
    }

    pub fn new_timed(start: i64, end: i64, tz: Tz) -> Self {
        let mut f = Form::blank(Target::New);
        f.start = TextInput::new(fmt_instant(start, tz));
        f.end = TextInput::new(fmt_instant(end, tz));
        f
    }

    pub fn new_all_day(first: NaiveDate, last: NaiveDate) -> Self {
        let mut f = Form::blank(Target::New);
        f.all_day = true;
        f.start = TextInput::new(first.to_string());
        f.end = TextInput::new(last.to_string());
        f
    }

    fn fill(&mut self, title: &str, all_day: bool, start: i64, end: i64, tz: Tz) {
        self.title = TextInput::new(title);
        self.all_day = all_day;
        if all_day {
            self.start = TextInput::new(time::floating_date(start).to_string());
            self.end = TextInput::new(last_day(start, end).to_string());
        } else {
            self.start = TextInput::new(fmt_instant(start, tz));
            self.end = TextInput::new(fmt_instant(end, tz));
        }
    }

    pub fn edit_series(ev: Event, tz: Tz) -> Self {
        let mut f = Form::blank(Target::Series(ev.clone()));
        f.fill(&ev.title, ev.all_day, ev.start, ev.end, tz);
        f.category = TextInput::new(ev.category.unwrap_or_default());
        f.location = TextInput::new(ev.location.unwrap_or_default());
        f.reminder = TextInput::new(ev.reminder.map(time::format_minutes).unwrap_or_default());
        f.repeat = TextInput::new(ev.rrule.unwrap_or_default());
        f.notes = TextInput::new(ev.description);
        f
    }

    pub fn edit_occurrence(ev: Event, occ: &Occurrence, tz: Tz) -> Self {
        let original_start = occ.original_start.unwrap_or(occ.start);
        let mut f = Form::blank(Target::Occurrence {
            event: ev,
            original_start,
        });
        f.fill(&occ.title, occ.all_day, occ.start, occ.end, tz);
        f.category = TextInput::new(occ.category.clone().unwrap_or_default());
        f.location = TextInput::new(occ.location.clone().unwrap_or_default());
        f.notes = TextInput::new(occ.description.clone());
        f
    }

    pub fn heading(&self) -> &'static str {
        match self.target {
            Target::New => " New event ",
            Target::Series(ref e) if e.is_recurring() => " Edit series ",
            Target::Series(_) => " Edit event ",
            Target::Occurrence { .. } => " Edit occurrence ",
        }
    }

    /// Fields shown for this target; an occurrence can't change its
    /// recurrence or all-day-ness.
    pub fn fields(&self) -> Vec<Field> {
        FIELDS
            .into_iter()
            .filter(|f| {
                !matches!(self.target, Target::Occurrence { .. })
                    || !matches!(f, Field::Repeat | Field::AllDay | Field::Reminder)
            })
            .collect()
    }

    pub fn input(&self, field: Field) -> Option<&TextInput> {
        Some(match field {
            Field::Title => &self.title,
            Field::Start => &self.start,
            Field::End => &self.end,
            Field::AllDay => return None,
            Field::Category => &self.category,
            Field::Location => &self.location,
            Field::Reminder => &self.reminder,
            Field::Repeat => &self.repeat,
            Field::Notes => &self.notes,
        })
    }

    fn input_mut(&mut self, field: Field) -> Option<&mut TextInput> {
        Some(match field {
            Field::Title => &mut self.title,
            Field::Start => &mut self.start,
            Field::End => &mut self.end,
            Field::AllDay => return None,
            Field::Category => &mut self.category,
            Field::Location => &mut self.location,
            Field::Reminder => &mut self.reminder,
            Field::Repeat => &mut self.repeat,
            Field::Notes => &mut self.notes,
        })
    }

    pub fn hint(&self, field: Field) -> &'static str {
        match field {
            Field::Start if self.all_day => "YYYY-MM-DD, today, tomorrow",
            Field::Start => "YYYY-MM-DD HH:MM, tomorrow 9:00",
            Field::End if self.all_day => "last day, inclusive",
            Field::End => "time, or a length like 45m / 1h30m",
            Field::AllDay => "space to toggle",
            Field::Reminder => "before the start: 10m, 1h, 1d; empty for none",
            Field::Repeat => "RRULE, e.g. FREQ=WEEKLY;BYDAY=MO",
            _ => "",
        }
    }

    fn step(&mut self, by: isize) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.focus).unwrap_or(0) as isize;
        let n = fields.len() as isize;
        self.focus = fields[((at + by).rem_euclid(n)) as usize];
    }

    /// Switches between timed and all-day, converting the start/end text
    /// when it parses.
    fn toggle_all_day(&mut self, tz: Tz, settings: &Settings) {
        let now = time::now();
        let start = time::parse_when(&self.start.text, tz, now).ok();
        let end = time::parse_when(&self.end.text, tz, now).ok();
        self.all_day = !self.all_day;
        if self.all_day {
            if let Some(s) = start {
                let first = s.date(tz);
                self.start = TextInput::new(first.to_string());
                let last = match end {
                    // a timed end at midnight belongs to the day before
                    Some(When::Instant(e)) => time::local_date(e - 1, tz).max(first),
                    Some(When::Date(d)) => d.max(first),
                    None => first,
                };
                self.end = TextInput::new(last.to_string());
            }
        } else if let Some(s) = start {
            let start = match s {
                When::Instant(ts) => ts,
                When::Date(d) => time::resolve_local(
                    d.and_hms_opt(settings.default_start_hour, 0, 0)
                        .unwrap_or_default(),
                    tz,
                ),
            };
            self.start = TextInput::new(fmt_instant(start, tz));
            self.end = TextInput::new(fmt_instant(start + settings.default_duration, tz));
        }
    }

    /// Handles a key in insert mode. Returns true when the form wants to be
    /// submitted (Enter); Esc is handled by the caller.
    pub fn handle(&mut self, key: Key, tz: Tz, settings: &Settings) -> bool {
        self.error = None;
        match (key.code, key.mods.contains(KeyModifiers::SHIFT)) {
            (KeyCode::Enter, _) => return true,
            (KeyCode::Tab, false) | (KeyCode::Down, _) => self.step(1),
            (KeyCode::Tab, true) | (KeyCode::Up, _) => self.step(-1),
            (KeyCode::Char(' ' | 'x'), _) if self.focus == Field::AllDay => {
                self.toggle_all_day(tz, settings)
            }
            _ => {
                let focus = self.focus;
                if let Some(input) = self.input_mut(focus) {
                    input.handle(key);
                }
            }
        }
        false
    }

    fn opt(input: &TextInput) -> Option<String> {
        Some(input.text.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Validates the fields into the write to perform.
    pub fn submit(&self, tz: Tz, settings: &Settings, now: i64) -> Result<Submit> {
        let title = self.title.text.trim();
        ensure!(!title.is_empty(), "a title is required");
        let start = time::parse_when(&self.start.text, tz, now)
            .map_err(|_| anyhow::anyhow!("can't read start {:?}", self.start.text))?;
        let end_text = self.end.text.trim();

        let (start_ts, end_ts) = if self.all_day {
            let first = start.date(tz);
            let last = if end_text.is_empty() {
                first
            } else {
                time::parse_when(end_text, tz, now)
                    .map_err(|_| anyhow::anyhow!("can't read end {end_text:?}"))?
                    .date(tz)
            };
            ensure!(last >= first, "the last day is before the first");
            (time::floating(first), time::floating(last) + DAY)
        } else {
            let When::Instant(s) = start else {
                bail!("start needs a time (or switch on all day)");
            };
            let e = if end_text.is_empty() {
                s + settings.default_duration
            } else if let Ok(w) = time::parse_when(end_text, tz, now) {
                w.instant(tz)
            } else if let Ok(d) = time::parse_duration(end_text) {
                s + d
            } else {
                bail!("can't read end {end_text:?}");
            };
            ensure!(e >= s, "the end is before the start");
            (s, e)
        };

        let reminder = time::parse_reminder(&self.reminder.text)
            .map_err(|e| anyhow::anyhow!("reminder: {e:#}"))?;
        let apply = |ev: &mut Event| {
            ev.reminder = reminder;
            ev.title = title.to_string();
            ev.start = start_ts;
            ev.end = end_ts;
            ev.all_day = self.all_day;
            ev.category = Self::opt(&self.category);
            ev.location = Self::opt(&self.location);
            ev.description = self.notes.text.trim().to_string();
            ev.rrule = Self::opt(&self.repeat);
            if !ev.all_day && ev.tzid.is_none() {
                ev.tzid = Some(tz.name().to_string());
            }
        };
        Ok(match &self.target {
            Target::New => {
                let mut ev = Event::new(title, start_ts, end_ts);
                apply(&mut ev);
                Submit::Insert(ev)
            }
            Target::Series(ev) => {
                let mut ev = ev.clone();
                apply(&mut ev);
                Submit::Update(ev)
            }
            Target::Occurrence {
                event,
                original_start,
            } => {
                let mut x = Exception::modified(&event.id, *original_start);
                x.title = Some(title.to_string());
                x.description = Some(self.notes.text.trim().to_string());
                x.category = Self::opt(&self.category);
                x.location = Self::opt(&self.location);
                x.start = Some(start_ts);
                x.end = Some(end_ts);
                Submit::Exception(x)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn typed(input: &mut TextInput, s: &str) {
        for c in s.chars() {
            input.handle(Key::char(c));
        }
    }

    fn ctrl(c: char) -> Key {
        Key::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn text_input_editing() {
        let mut t = TextInput::new("héllo");
        t.handle(Key::plain(KeyCode::Left));
        t.handle(Key::plain(KeyCode::Backspace));
        assert_eq!(t.text, "hélo");
        t.handle(ctrl('a'));
        typed(&mut t, "¡");
        assert_eq!(t.text, "¡hélo");
        t.handle(ctrl('e'));
        typed(&mut t, " big world");
        t.handle(ctrl('w'));
        assert_eq!(t.text, "¡hélo big ");
        t.handle(ctrl('u'));
        assert_eq!(t, TextInput::default());
    }

    #[test]
    fn submit_timed_with_duration_end() {
        let s = Settings::default();
        let start = time::resolve_local(
            NaiveDate::from_ymd_opt(2026, 11, 10)
                .unwrap()
                .and_hms_opt(14, 0, 0)
                .unwrap(),
            BERLIN,
        );
        let mut f = Form::new_timed(start, start + 3600, BERLIN);
        typed(&mut f.title, "Dentist");
        f.end = TextInput::new("45m");
        let Submit::Insert(ev) = f.submit(BERLIN, &s, 0).unwrap() else {
            panic!()
        };
        assert_eq!(
            (ev.title.as_str(), ev.start, ev.end),
            ("Dentist", start, start + 2700)
        );
        assert_eq!(ev.tzid.as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn submit_errors() {
        let s = Settings::default();
        let mut f = Form::new_timed(0, 3600, BERLIN);
        assert!(
            f.submit(BERLIN, &s, 0)
                .unwrap_err()
                .to_string()
                .contains("title")
        );
        f.title = TextInput::new("x");
        f.start = TextInput::new("2026-11-10");
        assert!(
            f.submit(BERLIN, &s, 0)
                .unwrap_err()
                .to_string()
                .contains("needs a time")
        );
        f.start = TextInput::new("2026-11-10 10:00");
        f.end = TextInput::new("2026-11-10 09:00");
        assert!(f.submit(BERLIN, &s, 0).is_err());
    }

    #[test]
    fn all_day_toggle_converts() {
        let s = Settings::default();
        let mut f = Form::new_timed(0, 0, BERLIN);
        f.start = TextInput::new("2026-11-10 14:00");
        f.end = TextInput::new("2026-11-12 00:00");
        f.title = TextInput::new("Trip");
        f.focus = Field::AllDay;
        f.handle(Key::char(' '), BERLIN, &s);
        assert!(f.all_day);
        assert_eq!(
            (f.start.text.as_str(), f.end.text.as_str()),
            ("2026-11-10", "2026-11-11")
        );
        let Submit::Insert(ev) = f.submit(BERLIN, &s, 0).unwrap() else {
            panic!()
        };
        assert_eq!(ev.duration(), 2 * DAY);
        f.handle(Key::char(' '), BERLIN, &s);
        assert_eq!(f.start.text, "2026-11-10 09:00");
        assert_eq!(f.end.text, "2026-11-10 10:00");
    }

    #[test]
    fn reminder_round_trips() {
        let s = Settings::default();
        let mut f = Form::new_timed(3600, 7200, BERLIN);
        f.title = TextInput::new("Dentist");
        f.reminder = TextInput::new("1h30m");
        let Submit::Insert(mut ev) = f.submit(BERLIN, &s, 0).unwrap() else {
            panic!()
        };
        assert_eq!(ev.reminder, Some(90));
        ev.reminder = Some(1440);
        assert_eq!(Form::edit_series(ev, BERLIN).reminder.text, "1d");
        f.reminder = TextInput::new("soon");
        assert!(
            f.submit(BERLIN, &s, 0)
                .unwrap_err()
                .to_string()
                .contains("reminder")
        );
    }

    #[test]
    fn occurrence_hides_series_fields() {
        let mut ev = Event::new("standup", 0, 1800);
        ev.rrule = Some("FREQ=DAILY".into());
        let occ = Occurrence {
            event_id: ev.id.clone(),
            original_start: Some(DAY),
            title: "standup".into(),
            description: String::new(),
            category: None,
            location: None,
            start: DAY,
            end: DAY + 1800,
            all_day: false,
            status: Default::default(),
            reminder: None,
            modified: false,
        };
        let mut f = Form::edit_occurrence(ev, &occ, Tz::UTC);
        assert!(!f.fields().contains(&Field::Repeat));
        f.focus = Field::End;
        f.handle(Key::plain(KeyCode::Tab), Tz::UTC, &Settings::default());
        assert_eq!(f.focus, Field::Category, "skips All day");
        let Submit::Exception(x) = f.submit(Tz::UTC, &Settings::default(), 0).unwrap() else {
            panic!()
        };
        assert_eq!(
            (x.original_start, x.start, x.end),
            (DAY, Some(DAY), Some(DAY + 1800))
        );
    }
}
