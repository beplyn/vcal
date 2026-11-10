//! Quick add: one line of words into an event, for `:add` in the TUI and
//! `vcal add <words...>`.
//!
//! ```text
//! Dentist tomorrow 14:00 45m @health !1h
//! Standup mon 9:30-9:45 every mon,tue,wed,thu,fri @work
//! Trip 2026-11-20 3d
//! Lunch fri 12:30pm
//! ```
//!
//! Words that read as one of the following are taken out; the rest, in
//! order, is the title. `\word` keeps a word literal.
//!
//! - a day: `today`, `tomorrow`, `yesterday`, a weekday (`fri`, `friday`:
//!   the next one, today included), `2026-11-20`, `11-20`, `20 nov`, `nov 20`
//! - a time `14:00`, `9:30am`, `2pm`, or a range `14:00-15:30`
//! - a length `45m`, `1h30m`, `2d`
//! - `@category`, `!10m` (a reminder that long before)
//! - `daily`, `weekly`, `monthly`, `yearly`, `every mon,wed`, `every day`
//!
//! Without a time the event is all-day. The day defaults to the given base
//! date (the cursor in the TUI, today on the command line).

use anyhow::{Result, bail, ensure};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Weekday};
use chrono_tz::Tz;

use crate::model::Event;
use crate::time::{self, DAY};

fn weekday(word: &str) -> Option<Weekday> {
    let w = word.to_ascii_lowercase();
    let w = w.trim_end_matches(['.', ',']);
    let all = [
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
        ("sunday", Weekday::Sun),
    ];
    all.iter()
        .find(|(name, _)| w.len() >= 3 && name.starts_with(w))
        .map(|(_, d)| *d)
}

fn month(word: &str) -> Option<u32> {
    let w = word.to_ascii_lowercase();
    let w = w.trim_end_matches(['.', ',']);
    let names = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    (w.len() >= 3)
        .then(|| names.iter().position(|n| n.starts_with(w)))
        .flatten()
        .map(|i| i as u32 + 1)
}

/// `14:00`, `9:30`, `9am`, `2:30pm`, `12pm`.
fn clock(word: &str) -> Option<NaiveTime> {
    let w = word.to_ascii_lowercase();
    let (body, pm) = match (w.strip_suffix("am"), w.strip_suffix("pm")) {
        (Some(b), _) => (b, Some(false)),
        (_, Some(b)) => (b, Some(true)),
        _ => (w.as_str(), None),
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None if pm.is_some() => (body.parse::<u32>().ok()?, 0),
        _ => return None,
    };
    if !(1..=2).contains(&body.split(':').next()?.len()) {
        return None;
    }
    let h = match pm {
        Some(_) if !(1..=12).contains(&h) => return None,
        Some(true) if h != 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

fn duration(word: &str) -> Option<i64> {
    // a bare number is a title word here ("Room 4"), not minutes
    if !word.chars().any(|c| c.is_ascii_alphabetic())
        || !word.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    time::parse_duration(word).ok().filter(|d| *d > 0)
}

/// The first date on or after `from` with this month and day.
fn upcoming(from: NaiveDate, month: u32, day: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(from.year(), month, day)
        .filter(|d| *d >= from)
        .or_else(|| NaiveDate::from_ymd_opt(from.year() + 1, month, day))
}

fn byday(days: &[Weekday]) -> String {
    days.iter()
        .map(|d| match d {
            Weekday::Mon => "MO",
            Weekday::Tue => "TU",
            Weekday::Wed => "WE",
            Weekday::Thu => "TH",
            Weekday::Fri => "FR",
            Weekday::Sat => "SA",
            Weekday::Sun => "SU",
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Parses a quick-add line into an unsaved event. `base` is the day when
/// none is given; `today` anchors "tomorrow", weekdays and month-day dates.
pub fn parse(
    input: &str,
    base: NaiveDate,
    today: NaiveDate,
    tz: Tz,
    default_duration: i64,
) -> Result<Event> {
    let words: Vec<&str> = input.split_whitespace().collect();
    let mut title: Vec<String> = Vec::new();
    let mut date: Option<NaiveDate> = None;
    let mut start: Option<NaiveTime> = None;
    let mut end: Option<NaiveTime> = None;
    let mut length: Option<i64> = None;
    let mut category = None;
    let mut reminder = None;
    let mut rrule: Option<String> = None;

    let set_date = |d: NaiveDate, date: &mut Option<NaiveDate>| -> Result<()> {
        ensure!(
            date.is_none(),
            "two days given ({} and {d})",
            date.unwrap_or(d)
        );
        *date = Some(d);
        Ok(())
    };

    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let lower = w.to_ascii_lowercase();
        let next = words.get(i + 1).copied();
        i += 1;
        if let Some(lit) = w.strip_prefix('\\') {
            title.push(lit.to_string());
            continue;
        }
        if let Some(c) = w.strip_prefix('@').filter(|c| !c.is_empty()) {
            category = Some(c.to_string());
            continue;
        }
        if let Some(r) = w.strip_prefix('!').filter(|r| !r.is_empty())
            && let Ok(m) = time::parse_reminder(r)
        {
            reminder = m;
            continue;
        }
        match lower.as_str() {
            "today" => {
                set_date(today, &mut date)?;
                continue;
            }
            "tomorrow" | "tmrw" => {
                set_date(today + Duration::days(1), &mut date)?;
                continue;
            }
            "yesterday" => {
                set_date(today - Duration::days(1), &mut date)?;
                continue;
            }
            "daily" | "weekly" | "monthly" | "yearly" => {
                rrule = Some(format!("FREQ={}", lower.to_ascii_uppercase()));
                continue;
            }
            "every" => {
                if let Some(n) = next {
                    let n = n.to_ascii_lowercase();
                    let freq = match n.as_str() {
                        "day" => Some("FREQ=DAILY".to_string()),
                        "week" => Some("FREQ=WEEKLY".to_string()),
                        "month" => Some("FREQ=MONTHLY".to_string()),
                        "year" => Some("FREQ=YEARLY".to_string()),
                        "weekday" | "weekdays" => {
                            Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR".to_string())
                        }
                        list => {
                            let days: Option<Vec<Weekday>> = list
                                .split(',')
                                .filter(|s| !s.is_empty())
                                .map(weekday)
                                .collect();
                            days.filter(|d| !d.is_empty())
                                .map(|d| format!("FREQ=WEEKLY;BYDAY={}", byday(&d)))
                        }
                    };
                    if let Some(f) = freq {
                        rrule = Some(f);
                        i += 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        if let Some(d) = weekday(w) {
            let ahead = (7 + d.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
            set_date(today + Duration::days(i64::from(ahead)), &mut date)?;
            continue;
        }
        if let Ok(d) = NaiveDate::parse_from_str(w, "%Y-%m-%d") {
            set_date(d, &mut date)?;
            continue;
        }
        if let Some((m, d)) = w.split_once('-')
            && m.len() <= 2
            && d.len() <= 2
            && let (Ok(m), Ok(d)) = (m.parse::<u32>(), d.parse::<u32>())
            && let Some(found) = upcoming(today, m, d)
        {
            set_date(found, &mut date)?;
            continue;
        }
        // "20 nov" and "nov 20"
        if let Some(n) = next {
            let pair = match (
                w.trim_end_matches('.').parse::<u32>(),
                month(n),
                month(w),
                n.trim_end_matches([',', '.']).parse::<u32>(),
            ) {
                (Ok(d), Some(m), _, _) => Some((m, d)),
                (_, _, Some(m), Ok(d)) => Some((m, d)),
                _ => None,
            };
            if let Some((m, d)) = pair
                && let Some(found) = upcoming(today, m, d)
            {
                set_date(found, &mut date)?;
                i += 1;
                continue;
            }
        }
        if let Some((a, b)) = w.split_once('-')
            && let (Some(a), Some(b)) = (clock(a), clock(b))
        {
            ensure!(start.is_none(), "two times given");
            (start, end) = (Some(a), Some(b));
            continue;
        }
        if let Some(t) = clock(w) {
            ensure!(start.is_none(), "two times given");
            start = Some(t);
            continue;
        }
        if let Some(d) = duration(w) {
            ensure!(length.is_none(), "two lengths given");
            length = Some(d);
            continue;
        }
        title.push(w.to_string());
    }

    ensure!(
        !title.is_empty(),
        "no title (try: Dentist tomorrow 14:00 45m @health)"
    );
    let day = date.unwrap_or(base);
    let mut ev = match start {
        Some(s) => {
            ensure!(
                end.is_none() || length.is_none(),
                "give an end time or a length, not both"
            );
            let from = time::resolve_local(day.and_time(s), tz);
            let to = match (end, length) {
                (Some(e), _) => {
                    // 23:00-01:00 runs past midnight
                    let end_day = if e <= s { day + Duration::days(1) } else { day };
                    time::resolve_local(end_day.and_time(e), tz)
                }
                (None, Some(d)) => from + d,
                (None, None) => from + default_duration,
            };
            let mut ev = Event::new(title.join(" "), from, to);
            ev.tzid = Some(tz.name().to_string());
            ev
        }
        None => {
            let days = match length {
                Some(d) if d % DAY == 0 => d / DAY,
                Some(_) => {
                    bail!("an all-day event lasts whole days (2d); add a time for a timed one")
                }
                None => 1,
            };
            let from = time::floating(day);
            let mut ev = Event::new(title.join(" "), from, from + days * DAY);
            ev.all_day = true;
            ev
        }
    };
    ev.category = category;
    ev.reminder = reminder;
    ev.rrule = rrule;
    Ok(ev)
}

/// A one-line description of a parsed event, for confirmations.
pub fn describe(ev: &Event, tz: Tz, clock_24h: bool) -> String {
    let fmt = |ts| {
        let dt = time::in_tz(ts, tz);
        if clock_24h {
            dt.format("%a %-d %b %H:%M").to_string()
        } else {
            dt.format("%a %-d %b %-I:%M%P").to_string()
        }
    };
    let when = if ev.all_day {
        let first = time::floating_date(ev.start);
        let days = (ev.end - ev.start) / DAY;
        if days > 1 {
            format!("{} ({days} days)", first.format("%a %-d %b"))
        } else {
            first.format("%a %-d %b").to_string()
        }
    } else {
        format!(
            "{} ({})",
            fmt(ev.start),
            time::format_minutes(ev.duration() / 60)
        )
    };
    let mut out = format!("\"{}\" {when}", ev.title);
    if let Some(c) = &ev.category {
        out.push_str(&format!(" @{c}"));
    }
    if ev.rrule.is_some() {
        out.push_str(" ↻");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    // Tuesday
    fn today() -> NaiveDate {
        d(2026, 11, 10)
    }

    fn q(s: &str) -> Event {
        parse(s, today(), today(), BERLIN, 3600).unwrap()
    }

    fn local(ev: &Event) -> (String, String) {
        let f = |ts| time::in_tz(ts, BERLIN).format("%Y-%m-%d %H:%M").to_string();
        (f(ev.start), f(ev.end))
    }

    #[test]
    fn timed() {
        let ev = q("Dentist tomorrow 14:00 45m @health !1h");
        assert_eq!(ev.title, "Dentist");
        assert_eq!(
            local(&ev),
            ("2026-11-11 14:00".into(), "2026-11-11 14:45".into())
        );
        assert_eq!(ev.category.as_deref(), Some("health"));
        assert_eq!(ev.reminder, Some(60));
        assert!(!ev.all_day);

        let ev = q("Lunch with Sam fri 12:30pm");
        assert_eq!(ev.title, "Lunch with Sam");
        assert_eq!(local(&ev).0, "2026-11-13 12:30");
        assert_eq!(ev.duration(), 3600, "default length");

        assert_eq!(local(&q("Call 9am")).0, "2026-11-10 09:00");
        assert_eq!(local(&q("Call 12am")).0, "2026-11-10 00:00");
        let late = q("Party sat 23:00-01:00");
        assert_eq!(
            local(&late),
            ("2026-11-14 23:00".into(), "2026-11-15 01:00".into())
        );
    }

    #[test]
    fn days() {
        assert_eq!(
            local(&q("Thing tue 10:00")).0,
            "2026-11-10 10:00",
            "today counts"
        );
        assert_eq!(local(&q("Thing monday 10:00")).0, "2026-11-16 10:00");
        assert_eq!(local(&q("Thing 2027-01-02 10:00")).0, "2027-01-02 10:00");
        assert_eq!(local(&q("Thing 11-24 10:00")).0, "2026-11-24 10:00");
        assert_eq!(
            local(&q("Thing 01-05 10:00")).0,
            "2027-01-05 10:00",
            "next year"
        );
        assert_eq!(local(&q("Thing 24 nov 10:00")).0, "2026-11-24 10:00");
        assert_eq!(local(&q("Thing Dec 3 10:00")).0, "2026-12-03 10:00");
        // the base date applies when no day is given
        let ev = parse("Thing 10:00", d(2026, 12, 1), today(), BERLIN, 3600).unwrap();
        assert_eq!(local(&ev).0, "2026-12-01 10:00");
    }

    #[test]
    fn all_day_and_repeats() {
        let trip = q("Trip 2026-11-20 3d");
        assert!(trip.all_day);
        assert_eq!(time::floating_date(trip.start), d(2026, 11, 20));
        assert_eq!(trip.duration(), 3 * DAY);

        let s = q("Standup mon 9:30-9:45 every mon,tue,wed,thu,fri @work");
        assert_eq!(s.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"));
        assert_eq!(s.duration(), 15 * 60);
        assert_eq!(q("Rent monthly").rrule.as_deref(), Some("FREQ=MONTHLY"));
        assert_eq!(
            q("Gym every weekday 7:00").rrule.as_deref(),
            Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR")
        );
        assert_eq!(q("Plants every day").rrule.as_deref(), Some("FREQ=DAILY"));
    }

    #[test]
    fn titles_keep_ordinary_words() {
        assert_eq!(q("Room 4 sync 10:00").title, "Room 4 sync");
        assert_eq!(q("Read May notes").title, "Read May notes");
        assert_eq!(q("Every little thing").title, "Every little thing");
        assert_eq!(q("\\today show").title, "today show");
        assert_eq!(q("Mars landing").title, "Mars landing");
    }

    #[test]
    fn errors() {
        let e = |s: &str| {
            format!(
                "{:#}",
                parse(s, today(), today(), BERLIN, 3600).unwrap_err()
            )
        };
        assert!(e("tomorrow 10:00").contains("no title"));
        assert!(e("X today tomorrow").contains("two days"));
        assert!(e("X 10:00 11:00").contains("two times"));
        assert!(e("X 90m").contains("whole days"));
        assert!(e("X 10:00-11:00 1h").contains("not both"));
    }

    #[test]
    fn describes() {
        assert_eq!(
            describe(&q("Dentist tomorrow 14:00 45m @health"), BERLIN, true),
            "\"Dentist\" Wed 11 Nov 14:00 (45m) @health"
        );
    }
}
