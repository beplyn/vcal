//! The iCalendar (RFC 5545) writer behind `vcal export` and CalDAV uploads:
//! one VEVENT per event, recurring series with RRULE and EXDATE plus a
//! RECURRENCE-ID VEVENT for each changed occurrence, reminders as VALARMs,
//! and a VTIMEZONE for every zone used, so readers that don't know IANA
//! names still get the times right.

use std::collections::BTreeSet;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Offset};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

use crate::model::{Event, Exception, ExceptionKind};
use crate::time::{self, DAY};

pub const PRODID: &str = "-//beplyn//vcal//EN";

/// Escapes a TEXT value (RFC 5545 §3.3.11).
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// Appends a content line, folded at 75 octets without splitting a character.
fn line(out: &mut String, text: &str) {
    let mut width = 0;
    for c in text.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

fn utc_stamp(ts: i64) -> String {
    time::datetime_utc(ts).format("%Y%m%dT%H%M%SZ").to_string()
}

/// How an event's times are written.
#[derive(Clone, Copy)]
enum Form {
    Date,
    Zoned(Tz),
    Utc,
}

impl Form {
    fn of(ev: &Event) -> Form {
        if ev.all_day {
            return Form::Date;
        }
        match ev.tzid.as_deref().and_then(|z| z.parse::<Tz>().ok()) {
            Some(Tz::UTC) | None => Form::Utc,
            Some(tz) if tz.name() == "Etc/UTC" => Form::Utc,
            Some(tz) => Form::Zoned(tz),
        }
    }

    /// `NAME;PARAMS:value` for a stamp.
    fn prop(self, name: &str, ts: i64) -> String {
        match self {
            Form::Date => format!(
                "{name};VALUE=DATE:{}",
                time::floating_date(ts).format("%Y%m%d")
            ),
            Form::Zoned(tz) => format!(
                "{name};TZID={}:{}",
                tz.name(),
                time::in_tz(ts, tz).format("%Y%m%dT%H%M%S")
            ),
            Form::Utc => format!("{name}:{}", utc_stamp(ts)),
        }
    }
}

/// The parts of an event a VEVENT carries, after any occurrence changes.
struct Body<'a> {
    title: &'a str,
    description: &'a str,
    category: Option<&'a str>,
    location: Option<&'a str>,
    start: i64,
    end: i64,
}

fn vevent(out: &mut String, ev: &Event, body: &Body, recurrence_id: Option<i64>, now: i64) {
    let form = Form::of(ev);
    line(out, "BEGIN:VEVENT");
    line(out, &format!("UID:{}", escape(&ev.id)));
    line(out, &format!("DTSTAMP:{}", utc_stamp(now)));
    line(out, &format!("CREATED:{}", utc_stamp(ev.created)));
    line(out, &format!("LAST-MODIFIED:{}", utc_stamp(ev.updated)));
    if let Some(rid) = recurrence_id {
        line(out, &form.prop("RECURRENCE-ID", rid));
    }
    line(out, &form.prop("DTSTART", body.start));
    line(out, &form.prop("DTEND", body.end));
    line(out, &format!("SUMMARY:{}", escape(body.title)));
    if !body.description.is_empty() {
        line(out, &format!("DESCRIPTION:{}", escape(body.description)));
    }
    if let Some(l) = body.location {
        line(out, &format!("LOCATION:{}", escape(l)));
    }
    if let Some(c) = body.category {
        line(out, &format!("CATEGORIES:{}", escape(c)));
    }
    line(
        out,
        &format!("STATUS:{}", ev.status.as_str().to_ascii_uppercase()),
    );
    if recurrence_id.is_none()
        && let Some(rule) = &ev.rrule
    {
        line(out, &format!("RRULE:{rule}"));
    }
    if let Some(m) = ev.reminder {
        line(out, "BEGIN:VALARM");
        line(out, "ACTION:DISPLAY");
        line(out, &format!("DESCRIPTION:{}", escape(body.title)));
        line(out, &format!("TRIGGER:-PT{m}M"));
        line(out, "END:VALARM");
    }
    line(out, "END:VEVENT");
}

/// The VEVENTs of one event: the event (or series) and its changed occurrences.
pub fn event(out: &mut String, ev: &Event, exceptions: &[Exception], now: i64) {
    let form = Form::of(ev);
    let mut series = String::new();
    let base = Body {
        title: &ev.title,
        description: &ev.description,
        category: ev.category.as_deref(),
        location: ev.location.as_deref(),
        start: ev.start,
        end: ev.end,
    };
    vevent(&mut series, ev, &base, None, now);
    if ev.is_recurring() {
        // EXDATEs go just before END:VEVENT of the series
        let cancelled: Vec<String> = exceptions
            .iter()
            .filter(|x| x.kind == ExceptionKind::Cancelled)
            .map(|x| form.prop("EXDATE", x.original_start))
            .collect();
        let end = series.len() - "END:VEVENT\r\n".len();
        let mut tail = String::new();
        for c in cancelled {
            line(&mut tail, &c);
        }
        series.insert_str(end, &tail);
    }
    out.push_str(&series);
    if !ev.is_recurring() {
        return;
    }
    for x in exceptions
        .iter()
        .filter(|x| x.kind == ExceptionKind::Modified)
    {
        let start = x.start.unwrap_or(x.original_start);
        let body = Body {
            title: x.title.as_deref().unwrap_or(&ev.title),
            description: x.description.as_deref().unwrap_or(&ev.description),
            category: x.category.as_deref().or(ev.category.as_deref()),
            location: x.location.as_deref().or(ev.location.as_deref()),
            start,
            end: x.end.unwrap_or(start + ev.duration()),
        };
        vevent(out, ev, &body, Some(x.original_start), now);
    }
}

/// A whole VCALENDAR with the given events and their exceptions.
pub fn calendar(items: &[(Event, Vec<Exception>)], now: i64) -> String {
    let mut out = String::new();
    line(&mut out, "BEGIN:VCALENDAR");
    line(&mut out, "VERSION:2.0");
    line(&mut out, &format!("PRODID:{PRODID}"));
    line(&mut out, "CALSCALE:GREGORIAN");

    let zones: BTreeSet<&str> = items
        .iter()
        .filter_map(|(ev, _)| match Form::of(ev) {
            Form::Zoned(tz) => Some(tz.name()),
            _ => None,
        })
        .collect();
    if !zones.is_empty() {
        let year = |ts: i64| time::datetime_utc(ts).year();
        let first = items
            .iter()
            .map(|(e, x)| {
                x.iter()
                    .filter_map(|x| x.start)
                    .chain([e.start])
                    .min()
                    .unwrap_or(e.start)
            })
            .min()
            .map_or(year(now), year);
        let last = items
            .iter()
            .map(|(e, _)| year(e.end))
            .max()
            .unwrap_or(year(now));
        // open-ended series need rules well past today
        let years = (first - 1, last.max(year(now)) + 15);
        for name in zones {
            if let Ok(tz) = name.parse::<Tz>() {
                vtimezone(&mut out, tz, years);
            }
        }
    }

    for (ev, exceptions) in items {
        event(&mut out, ev, exceptions, now);
    }
    line(&mut out, "END:VCALENDAR");
    out
}

/// One offset change: the instant, the offset before and after, and
/// whether the new offset is daylight time.
struct Transition {
    at: i64,
    from: i32,
    to: i32,
    daylight: bool,
    name: Option<String>,
}

fn offset_at(tz: Tz, ts: i64) -> chrono_tz::TzOffset {
    *time::in_tz(ts, tz).offset()
}

fn seconds(o: &chrono_tz::TzOffset) -> i32 {
    o.fix().local_minus_utc()
}

/// Every offset change in `[from_year, to_year]`, found by stepping a day at
/// a time and bisecting the day it happens in.
fn transitions(tz: Tz, (from_year, to_year): (i32, i32)) -> (i32, Vec<Transition>) {
    let start = time::floating(NaiveDate::from_ymd_opt(from_year, 1, 1).unwrap_or_default());
    let end = time::floating(NaiveDate::from_ymd_opt(to_year + 1, 1, 1).unwrap_or_default());
    let initial = seconds(&offset_at(tz, start));
    let mut out = Vec::new();
    let mut t = start;
    let mut cur = initial;
    while t < end {
        let next = t + DAY;
        let off = seconds(&offset_at(tz, next));
        if off != cur {
            let (mut lo, mut hi) = (t, next);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if seconds(&offset_at(tz, mid)) == cur {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let after = offset_at(tz, hi);
            out.push(Transition {
                at: hi,
                from: cur,
                to: off,
                daylight: after.dst_offset().num_seconds() != 0,
                name: after.abbreviation().map(str::to_string),
            });
            cur = off;
        }
        t = next;
    }
    (initial, out)
}

fn offset_text(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    if a % 60 == 0 {
        format!("{sign}{:02}{:02}", a / 3600, a % 3600 / 60)
    } else {
        format!("{sign}{:02}{:02}{:02}", a / 3600, a % 3600 / 60, a % 60)
    }
}

/// Local wall-clock time just before a transition, as observances want it.
fn local_before(t: &Transition) -> NaiveDateTime {
    time::datetime_utc(t.at + i64::from(t.from)).naive_utc()
}

/// A VTIMEZONE listing the zone's transitions as observances with RDATEs
/// (one STANDARD or DAYLIGHT per distinct offset pair), plus one for the
/// offset in force before the first.
fn vtimezone(out: &mut String, tz: Tz, years: (i32, i32)) {
    let (initial, list) = transitions(tz, years);
    line(out, "BEGIN:VTIMEZONE");
    line(out, &format!("TZID:{}", tz.name()));
    line(out, &format!("X-LIC-LOCATION:{}", tz.name()));
    let first_day = NaiveDate::from_ymd_opt(years.0, 1, 1).unwrap_or_default();
    let base = offset_at(tz, time::floating(first_day));
    let initial_daylight = base.dst_offset().num_seconds() != 0;
    let kind = |d: bool| if d { "DAYLIGHT" } else { "STANDARD" };
    line(out, &format!("BEGIN:{}", kind(initial_daylight)));
    line(
        out,
        &format!("DTSTART:{}", first_day.format("%Y%m%dT000000")),
    );
    line(out, &format!("TZOFFSETFROM:{}", offset_text(initial)));
    line(out, &format!("TZOFFSETTO:{}", offset_text(initial)));
    if let Some(n) = base.abbreviation() {
        line(out, &format!("TZNAME:{n}"));
    }
    line(out, &format!("END:{}", kind(initial_daylight)));

    // (daylight, from, to, name, local starts)
    type Observance = (bool, i32, i32, Option<String>, Vec<NaiveDateTime>);
    let mut groups: Vec<Observance> = Vec::new();
    for t in &list {
        let local = local_before(t);
        match groups
            .iter_mut()
            .find(|g| g.0 == t.daylight && g.1 == t.from && g.2 == t.to && g.3 == t.name)
        {
            Some(g) => g.4.push(local),
            None => groups.push((t.daylight, t.from, t.to, t.name.clone(), vec![local])),
        }
    }
    for (daylight, from, to, name, dates) in groups {
        line(out, &format!("BEGIN:{}", kind(daylight)));
        line(
            out,
            &format!("DTSTART:{}", dates[0].format("%Y%m%dT%H%M%S")),
        );
        line(out, &format!("TZOFFSETFROM:{}", offset_text(from)));
        line(out, &format!("TZOFFSETTO:{}", offset_text(to)));
        if dates.len() > 1 {
            let rdates: Vec<String> = dates[1..]
                .iter()
                .map(|d| d.format("%Y%m%dT%H%M%S").to_string())
                .collect();
            for chunk in rdates.chunks(8) {
                line(out, &format!("RDATE:{}", chunk.join(",")));
            }
        }
        if let Some(n) = name {
            line(out, &format!("TZNAME:{n}"));
        }
        line(out, &format!("END:{}", kind(daylight)));
    }
    line(out, "END:VTIMEZONE");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics;
    use crate::model::Status;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn at(y: i32, m: u32, d: u32, h: u32) -> i64 {
        time::resolve_local(
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
            BERLIN,
        )
    }

    #[test]
    fn folds_at_75_octets() {
        let mut out = String::new();
        line(&mut out, &format!("SUMMARY:{}", "ä".repeat(60)));
        for l in out.split("\r\n") {
            assert!(l.len() <= 75, "{l:?} is {} octets", l.len());
        }
        let unfolded = out.replace("\r\n ", "");
        assert_eq!(unfolded, format!("SUMMARY:{}\r\n", "ä".repeat(60)));
    }

    #[test]
    fn round_trips() {
        let mut standup = Event::new("Standup; daily, mostly", at(2026, 10, 19, 9), 0);
        standup.end = standup.start + 1800;
        standup.tzid = Some("Europe/Berlin".into());
        standup.rrule = Some("FREQ=WEEKLY;BYDAY=MO".into());
        standup.description = "line one\nline two \\ backslash".into();
        standup.category = Some("work".into());
        standup.location = Some("Room 1".into());
        standup.reminder = Some(15);
        standup.status = Status::Tentative;
        let cancelled = Exception::cancelled(&standup.id, at(2026, 10, 26, 9));
        let mut moved = Exception::modified(&standup.id, at(2026, 11, 2, 9));
        moved.title = Some("Late standup".into());
        moved.start = Some(at(2026, 11, 2, 10));
        moved.end = Some(at(2026, 11, 2, 11));

        let mut trip = Event::new(
            "Trip",
            time::floating(NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()),
            0,
        );
        trip.end = trip.start + 3 * DAY;
        trip.all_day = true;

        let mut utc = Event::new("Call", 1_800_000_000, 1_800_003_600);
        utc.tzid = Some("UTC".into());

        let text = calendar(
            &[
                (standup.clone(), vec![cancelled.clone(), moved.clone()]),
                (trip.clone(), vec![]),
                (utc.clone(), vec![]),
            ],
            time::now(),
        );
        assert!(text.contains("BEGIN:VTIMEZONE\r\nTZID:Europe/Berlin"));
        assert!(text.contains("EXDATE;TZID=Europe/Berlin:20261026T090000"));

        let p = ics::parse(&text, chrono_tz::Asia::Tokyo).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.events.len(), 3);
        let s = &p.events[0];
        assert_eq!(
            (&s.id, &s.title, &s.description),
            (&standup.id, &standup.title, &standup.description)
        );
        assert_eq!((s.start, s.end), (standup.start, standup.end));
        assert_eq!(s.tzid, standup.tzid);
        assert_eq!(s.rrule, standup.rrule);
        assert_eq!(
            (&s.category, &s.location),
            (&standup.category, &standup.location)
        );
        assert_eq!((s.reminder, s.status), (Some(15), Status::Tentative));
        let t = &p.events[1];
        assert!(t.all_day);
        assert_eq!((t.start, t.end), (trip.start, trip.end));
        assert_eq!((p.events[2].start, p.events[2].end), (utc.start, utc.end));

        assert_eq!(p.exceptions.len(), 2);
        assert_eq!(p.exceptions[0], cancelled);
        let m = &p.exceptions[1];
        assert_eq!(m.original_start, moved.original_start);
        assert_eq!((m.start, m.end), (moved.start, moved.end));
        assert_eq!(m.title, moved.title);
    }

    #[test]
    fn vtimezone_has_the_transitions() {
        let (initial, list) = transitions(BERLIN, (2026, 2026));
        assert_eq!(initial, 3600);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].at, at(2026, 3, 29, 3));
        assert!(list[0].daylight);
        assert_eq!(
            local_before(&list[0]).format("%m-%d %H:%M").to_string(),
            "03-29 02:00"
        );
        assert_eq!((list[1].from, list[1].to), (7200, 3600));
        assert_eq!(
            local_before(&list[1]).format("%m-%d %H:%M").to_string(),
            "10-25 03:00"
        );
        let (_, none) = transitions(chrono_tz::Asia::Tokyo, (2020, 2030));
        assert!(none.is_empty());
    }
}
