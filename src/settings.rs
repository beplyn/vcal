//! User preferences: the `[general]` and `[views]` parts of config.toml.

use chrono::{DateTime, Weekday};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum View {
    #[default]
    Month,
    Week,
    Day,
    /// Upcoming events as a list, day by day
    Agenda,
}

impl View {
    pub const ALL: [View; 4] = [View::Month, View::Week, View::Day, View::Agenda];

    pub fn name(self) -> &'static str {
        match self {
            View::Month => "Month",
            View::Week => "Week",
            View::Day => "Day",
            View::Agenda => "Agenda",
        }
    }

    pub fn parse(s: &str) -> Option<View> {
        View::ALL
            .into_iter()
            .find(|v| v.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub week_start: Weekday,
    pub clock_24h: bool,
    /// Seconds.
    pub default_duration: i64,
    pub start_view: View,
    /// Week/day grid granularity: 15, 30 or 60.
    pub slot_minutes: u32,
    /// Hours shown in week/day views, `[first, last)`.
    pub first_hour: u32,
    pub last_hour: u32,
    /// Hour new timed events start at in month view when no slot is selected.
    pub default_start_hour: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            week_start: Weekday::Mon,
            clock_24h: true,
            default_duration: 3600,
            start_view: View::Month,
            slot_minutes: 30,
            first_hour: 0,
            last_hour: 24,
            default_start_hour: 9,
        }
    }
}

impl Settings {
    pub fn time(&self, dt: DateTime<Tz>) -> String {
        if self.clock_24h {
            dt.format("%H:%M").to_string()
        } else {
            dt.format("%-I:%M%P").to_string()
        }
    }

    /// Short hour label for the time gutter.
    pub fn hour_label(&self, hour: u32) -> String {
        if self.clock_24h {
            format!("{hour:02}:00")
        } else {
            let h12 = if hour.is_multiple_of(12) {
                12
            } else {
                hour % 12
            };
            format!("{h12}{}", if hour < 12 { "am" } else { "pm" })
        }
    }

    pub fn slots_per_day(&self) -> u32 {
        24 * 60 / self.slot_minutes
    }

    pub fn first_slot(&self) -> u32 {
        self.first_hour * 60 / self.slot_minutes
    }

    /// One past the last visible slot.
    pub fn end_slot(&self) -> u32 {
        self.last_hour * 60 / self.slot_minutes
    }
}

impl View {
    /// Whether the view is laid out by whole days (month, agenda) rather
    /// than by time slots (week, day): selections, moves and new events
    /// then work in days.
    pub fn by_days(self) -> bool {
        matches!(self, View::Month | View::Agenda)
    }
}
