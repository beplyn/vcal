//! A small iCalendar (RFC 5545) reader for `vcal import`: VEVENTs with
//! SUMMARY, DESCRIPTION, LOCATION, CATEGORIES, STATUS, DTSTART, DTEND or
//! DURATION, RRULE, EXDATE, RECURRENCE-ID overrides, and the first VALARM
//! with a TRIGGER relative to the start, as the reminder. TZIDs must be IANA
//! names; anything else is read in the local zone.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{NaiveDate, NaiveDateTime};
use chrono_tz::Tz;

use crate::model::{Event, Exception, Status};
use crate::time::{self, DAY};

/// What a file turned into, ready for [`crate::db::Db::import`].
#[derive(Debug, Default)]
pub struct Parsed {
    pub events: Vec<Event>,
    pub exceptions: Vec<Exception>,
    /// Components skipped, with the reason.
    pub warnings: Vec<String>,
}

struct Prop {
    name: String,
    params: HashMap<String, String>,
    value: String,
}

/// A DTSTART-like value: an instant (with the zone it was written in, if
/// any) or a floating date.
#[derive(Clone, Copy)]
enum Stamp {
    At(i64, Option<Tz>),
    Day(NaiveDate),
}

impl Stamp {
    /// Stored form: epoch for instants, floating midnight for dates.
    fn ts(self) -> i64 {
        match self {
            Stamp::At(ts, _) => ts,
            Stamp::Day(d) => time::floating(d),
        }
    }
}

/// Joins folded lines (CRLF followed by a space or tab).
fn unfold(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match (line.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(cont), Some(last)) => last.push_str(cont),
            _ if line.is_empty() => {}
            _ => lines.push(line.to_string()),
        }
    }
    lines
}

fn parse_prop(line: &str) -> Option<Prop> {
    // The value starts at the first ':' outside a quoted parameter.
    let mut quoted = false;
    let colon = line.char_indices().find_map(|(i, c)| match c {
        '"' => {
            quoted = !quoted;
            None
        }
        ':' if !quoted => Some(i),
        _ => None,
    })?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_uppercase(),
                v.trim_matches('"').to_string(),
            )
        })
        .collect();
    Some(Prop {
        name,
        params,
        value: value.to_string(),
    })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Splits a list value on unescaped commas.
fn split_list(s: &str) -> Vec<String> {
    let mut items = vec![String::new()];
    let mut escaped = false;
    for c in s.chars() {
        match c {
            ',' if !escaped => items.push(String::new()),
            _ => items.last_mut().unwrap().push(c),
        }
        escaped = c == '\\' && !escaped;
    }
    items
        .iter()
        .map(|i| unescape(i.trim()))
        .filter(|i| !i.is_empty())
        .collect()
}

fn parse_stamp(value: &str, params: &HashMap<String, String>, local: Tz) -> Result<Stamp> {
    let v = value.trim();
    if params
        .get("VALUE")
        .is_some_and(|t| t.eq_ignore_ascii_case("DATE"))
        || v.len() == 8
    {
        return Ok(Stamp::Day(
            NaiveDate::parse_from_str(v, "%Y%m%d").with_context(|| format!("bad date {v:?}"))?,
        ));
    }
    if let Some(utc) = v.strip_suffix(['Z', 'z']) {
        let naive = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S")
            .with_context(|| format!("bad date-time {v:?}"))?;
        return Ok(Stamp::At(naive.and_utc().timestamp(), None));
    }
    let tz = params.get("TZID").and_then(|z| z.parse::<Tz>().ok());
    let naive = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .with_context(|| format!("bad date-time {v:?}"))?;
    let zone = tz.unwrap_or(local);
    Ok(Stamp::At(time::resolve_local(naive, zone), Some(zone)))
}

/// ISO 8601 / RFC 5545 duration: `P1D`, `PT1H30M`, `P2W`, `-PT15M`.
fn parse_ics_duration(value: &str) -> Result<i64> {
    let v = value.trim();
    let (sign, v) = match v.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let body = v
        .strip_prefix(['P', 'p'])
        .ok_or_else(|| anyhow!("bad duration {value:?}"))?;
    let mut total = 0;
    let mut num = String::new();
    let mut in_time = false;
    for c in body.chars() {
        match c.to_ascii_uppercase() {
            d if d.is_ascii_digit() => num.push(d),
            'T' => in_time = true,
            unit => {
                let n: i64 = num
                    .parse()
                    .with_context(|| format!("bad duration {value:?}"))?;
                num.clear();
                total += n * match (unit, in_time) {
                    ('W', false) => 7 * DAY,
                    ('D', false) => DAY,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => bail!("bad duration {value:?}"),
                };
            }
        }
    }
    Ok(sign * total)
}

/// Turns iCalendar text into events and exceptions. Events keep their UID as
/// id, so importing the same file twice updates rather than duplicates.
pub fn parse(input: &str, local: Tz) -> Result<Parsed> {
    if !input.to_ascii_uppercase().contains("BEGIN:VCALENDAR") {
        bail!("not an iCalendar file (no BEGIN:VCALENDAR)");
    }
    let mut parsed = Parsed::default();
    let mut stack: Vec<String> = Vec::new();
    let mut current: Option<Vec<Prop>> = None;
    let mut reminder: Option<i64> = None;
    for line in unfold(input) {
        let Some(prop) = parse_prop(&line) else {
            continue;
        };
        match prop.name.as_str() {
            "BEGIN" => {
                let comp = prop.value.trim().to_ascii_uppercase();
                if comp == "VEVENT" && stack.last().is_some_and(|c| c == "VCALENDAR") {
                    current = Some(Vec::new());
                    reminder = None;
                }
                stack.push(comp);
            }
            "END" => {
                if stack.pop().as_deref() == Some("VEVENT")
                    && let Some(props) = current.take()
                    && let Err(err) = vevent(props, reminder.take(), local, &mut parsed)
                {
                    parsed.warnings.push(format!("skipped an event: {err:#}"));
                }
            }
            "TRIGGER"
                if reminder.is_none()
                    && stack.last().is_some_and(|c| c == "VALARM")
                    && stack.iter().rev().nth(1).is_some_and(|c| c == "VEVENT") =>
            {
                reminder = trigger_minutes(&prop);
            }
            // Other properties of a nested component (VALARM) don't belong to the event.
            _ if stack.last().is_some_and(|c| c == "VEVENT") => {
                if let Some(props) = current.as_mut() {
                    props.push(prop);
                }
            }
            _ => {}
        }
    }
    Ok(parsed)
}

/// Minutes before the start for a TRIGGER relative to it (`-PT15M`); `None`
/// for absolute triggers, ones relative to the end, or ones after the start.
fn trigger_minutes(prop: &Prop) -> Option<i64> {
    let related_end = prop
        .params
        .get("RELATED")
        .is_some_and(|r| r.eq_ignore_ascii_case("END"));
    let absolute = prop
        .params
        .get("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"));
    if related_end || absolute {
        return None;
    }
    let secs = parse_ics_duration(prop.value.trim()).ok()?;
    (secs <= 0).then_some(-secs / 60)
}

fn vevent(props: Vec<Prop>, reminder: Option<i64>, local: Tz, parsed: &mut Parsed) -> Result<()> {
    let one = |name: &str| props.iter().find(|p| p.name == name);
    let text = |name: &str| {
        one(name)
            .map(|p| unescape(&p.value))
            .filter(|s| !s.is_empty())
    };

    let dtstart = one("DTSTART").ok_or_else(|| anyhow!("no DTSTART"))?;
    let start = parse_stamp(&dtstart.value, &dtstart.params, local)?;
    let all_day = matches!(start, Stamp::Day(_));
    let end = match (one("DTEND"), one("DURATION")) {
        (Some(p), _) => parse_stamp(&p.value, &p.params, local)?.ts(),
        (None, Some(p)) => start.ts() + parse_ics_duration(&p.value)?,
        // RFC 5545: a date lasts one day, a date-time is instantaneous
        (None, None) => start.ts() + if all_day { DAY } else { 0 },
    };
    let status = match text("STATUS") {
        Some(s) => s.parse::<Status>().unwrap_or_default(),
        None => Status::Confirmed,
    };
    let uid = text("UID");

    if let Some(rid) = one("RECURRENCE-ID") {
        let uid = uid.ok_or_else(|| anyhow!("override without UID"))?;
        let original = parse_stamp(&rid.value, &rid.params, local)?.ts();
        let mut exc = if status == Status::Cancelled {
            Exception::cancelled(uid, original)
        } else {
            Exception::modified(uid, original)
        };
        if exc.kind == crate::model::ExceptionKind::Modified {
            exc.title = text("SUMMARY");
            exc.description = text("DESCRIPTION");
            exc.location = text("LOCATION");
            exc.category = one("CATEGORIES").and_then(|p| split_list(&p.value).into_iter().next());
            exc.start = Some(start.ts());
            exc.end = Some(end.max(start.ts()));
        }
        parsed.exceptions.push(exc);
        return Ok(());
    }

    let mut ev = Event::new(
        text("SUMMARY").unwrap_or_else(|| "(untitled)".into()),
        start.ts(),
        end,
    );
    if let Some(uid) = uid {
        ev.id = uid;
    }
    ev.description = text("DESCRIPTION").unwrap_or_default();
    ev.location = text("LOCATION");
    ev.category = one("CATEGORIES").and_then(|p| split_list(&p.value).into_iter().next());
    ev.all_day = all_day;
    ev.status = status;
    ev.reminder = reminder;
    if all_day && ev.end <= ev.start {
        ev.end = ev.start + DAY;
    }
    ev.end = ev.end.max(ev.start);
    if let Stamp::At(_, zone) = start {
        ev.tzid = Some(zone.unwrap_or(local).name().to_string());
    }
    ev.rrule = one("RRULE").map(|p| p.value.clone());

    for p in props.iter().filter(|p| p.name == "EXDATE") {
        for value in p.value.split(',') {
            let original = parse_stamp(value, &p.params, local)?.ts();
            parsed
                .exceptions
                .push(Exception::cancelled(ev.id.clone(), original));
        }
    }
    parsed.events.push(ev);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:standup@example\r\nSUMMARY:Stand\r\n  up\r\n\
DTSTART;TZID=Europe/Berlin:20261019T090000\r\nDURATION:PT30M\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\r\nEXDATE;TZID=Europe/Berlin:20261026T090000\r\n\
CATEGORIES:work,daily\r\nDESCRIPTION:Line one\\nLine\\, two\r\n\
BEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:ignored\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
BEGIN:VALARM\r\nTRIGGER;RELATED=END:-PT5M\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:standup@example\r\nRECURRENCE-ID;TZID=Europe/Berlin:20261102T090000\r\n\
DTSTART;TZID=Europe/Berlin:20261102T100000\r\nDTEND;TZID=Europe/Berlin:20261102T103000\r\n\
SUMMARY:Late standup\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:trip\r\nSUMMARY:Trip\r\nDTSTART;VALUE=DATE:20261120\r\n\
DTEND;VALUE=DATE:20261123\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_sample() {
        let p = parse(SAMPLE, Tz::UTC).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.events.len(), 2);
        let standup = &p.events[0];
        assert_eq!(standup.id, "standup@example");
        assert_eq!(standup.title, "Stand up");
        assert_eq!(standup.description, "Line one\nLine, two");
        assert_eq!(standup.category.as_deref(), Some("work"));
        assert_eq!(standup.duration(), 1800);
        assert_eq!(standup.tzid.as_deref(), Some("Europe/Berlin"));
        assert_eq!(standup.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
        assert_eq!(standup.reminder, Some(15));

        let trip = &p.events[1];
        assert!(trip.all_day);
        assert_eq!(trip.reminder, None);
        assert_eq!(trip.duration(), 3 * DAY);
        assert_eq!(
            time::floating_date(trip.start),
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );

        assert_eq!(p.exceptions.len(), 2);
        assert_eq!(p.exceptions[0].kind, crate::model::ExceptionKind::Cancelled);
        assert_eq!(p.exceptions[1].title.as_deref(), Some("Late standup"));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_ics_duration("PT1H30M").unwrap(), 5400);
        assert_eq!(parse_ics_duration("P1W").unwrap(), 7 * DAY);
        assert_eq!(parse_ics_duration("-P1DT2H").unwrap(), -(DAY + 7200));
        assert!(parse_ics_duration("1H").is_err());
    }

    #[test]
    fn rejects_non_ics() {
        assert!(parse("hello", Tz::UTC).is_err());
    }
}
