//! RRULE expansion. Series expand over floating wall-clock times, which are
//! then placed in the event's zone, so they keep their wall-clock time across
//! DST. A time a DST change skips moves forward past the gap, as DTSTART does,
//! rather than dropping the occurrence. All-day series are floating already.

use anyhow::{Context, Result, anyhow};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::Tz;
use rrule::{RRule, RRuleSet, Unvalidated};

use crate::model::Event;
use crate::time;

/// Hard cap on occurrences produced per series per query.
const MAX_OCCURRENCES: u16 = u16::MAX;

pub fn zone(ev: &Event) -> Tz {
    if ev.all_day {
        return Tz::UTC;
    }
    ev.tzid
        .as_deref()
        .and_then(|z| z.parse().ok())
        .unwrap_or(Tz::UTC)
}

/// Canonical RRULE text: no `RRULE:` prefix, and `UNTIL` in UTC as RFC 5545
/// requires for zoned starts. A date-only or floating `UNTIL` is read in the
/// event's zone, a date meaning the end of that day.
pub fn normalize(rule: &str, all_day: bool, tz: Tz) -> Result<String> {
    let body = rule.trim();
    let body = match body.get(..6) {
        Some(p) if p.eq_ignore_ascii_case("RRULE:") => &body[6..],
        _ => body,
    };
    let zone = if all_day { Tz::UTC } else { tz };
    body.split(';')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let Some((key, value)) = part.split_once('=') else {
                return Err(anyhow!("bad RRULE part {part:?}"));
            };
            let key = key.trim().to_ascii_uppercase();
            let value = value.trim();
            if key != "UNTIL" || value.ends_with(['Z', 'z']) {
                return Ok(format!("{key}={}", value.to_ascii_uppercase()));
            }
            let naive = if value.len() == 8 {
                NaiveDate::parse_from_str(value, "%Y%m%d")
                    .map(|d| d.and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()))
            } else {
                NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            }
            .with_context(|| format!("bad UNTIL {value:?}"))?;
            let until = time::resolve_local(naive, zone);
            Ok(format!(
                "UNTIL={}",
                time::datetime_utc(until).format("%Y%m%dT%H%M%SZ")
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|parts| parts.join(";"))
}

/// The rule set over floating wall-clock times (in `rrule::Tz::UTC`), or
/// `None` for a single event. [`place`] turns its dates into instants.
pub fn rule_set(ev: &Event) -> Result<Option<RRuleSet>> {
    let Some(rule) = ev.rrule.as_deref() else {
        return Ok(None);
    };
    let tz = zone(ev);
    let mut parsed: RRule<Unvalidated> = rule
        .parse()
        .with_context(|| format!("invalid RRULE {rule:?}"))?;
    if let Some(until) = parsed.get_until() {
        let until = floating(until.timestamp(), tz)?;
        parsed = parsed.until(until);
    }
    let set = parsed
        .build(floating(ev.start, tz)?)
        .with_context(|| format!("invalid RRULE {rule:?}"))?;
    Ok(Some(set))
}

/// The wall-clock time of `ts` in `tz`, as a floating time.
fn floating(ts: i64, tz: Tz) -> Result<chrono::DateTime<rrule::Tz>> {
    let local = tz
        .timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| anyhow!("{ts} out of range"))?
        .naive_local();
    Ok(local.and_utc().with_timezone(&rrule::Tz::UTC))
}

/// The instant a floating date from [`rule_set`] stands for in `tz`.
fn place(date: &chrono::DateTime<rrule::Tz>, tz: Tz) -> i64 {
    time::resolve_local(date.naive_utc(), tz)
}

/// End of the last occurrence for a bounded series (COUNT/UNTIL); `None` when
/// the series never ends or has no rule.
pub fn series_end(ev: &Event) -> Result<Option<i64>> {
    let Some(set) = rule_set(ev)? else {
        return Ok(None);
    };
    let bounded = set
        .get_rrule()
        .iter()
        .any(|r| r.get_count().is_some() || r.get_until().is_some());
    if !bounded {
        return Ok(None);
    }
    let result = set.all(MAX_OCCURRENCES);
    if result.limited {
        // Too many to enumerate; treat as unbounded rather than cut it short.
        return Ok(None);
    }
    let last = result.dates.last().map_or(ev.start, |d| place(d, zone(ev)));
    Ok(Some(last + ev.duration()))
}

/// Starts of occurrences with `lo <= start < hi`.
pub fn starts_between(ev: &Event, lo: i64, hi: i64) -> Result<Vec<i64>> {
    let Some(set) = rule_set(ev)? else {
        return Ok(if (lo..hi).contains(&ev.start) {
            vec![ev.start]
        } else {
            vec![]
        });
    };
    let tz = zone(ev);
    // Wall-clock and real time differ by the zone offset, under a day; widen
    // the floating window by that much and cut it exactly once placed.
    let mut starts: Vec<i64> = set
        .after(floating(lo.saturating_sub(time::DAY), Tz::UTC)?)
        .before(floating(hi.saturating_add(time::DAY), Tz::UTC)?)
        .all(MAX_OCCURRENCES)
        .dates
        .iter()
        .map(|d| place(d, tz))
        .filter(|s| (lo..hi).contains(s))
        .collect();
    // Two wall-clock times in one DST gap can land on the same instant.
    starts.dedup();
    Ok(starts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        let naive = NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap();
        time::resolve_local(naive, BERLIN)
    }

    fn weekly(rule: &str) -> Event {
        let start = at(2026, 10, 19, 9, 0); // a Monday
        let mut ev = Event::new("standup", start, start + 1800);
        ev.tzid = Some("Europe/Berlin".into());
        ev.rrule = Some(normalize(rule, false, BERLIN).unwrap());
        ev
    }

    #[test]
    fn normalizes_until_to_utc() {
        assert_eq!(
            normalize("RRULE:freq=weekly;until=20261130", false, BERLIN).unwrap(),
            "FREQ=WEEKLY;UNTIL=20261130T225959Z"
        );
        assert_eq!(
            normalize("FREQ=DAILY;UNTIL=20261130T090000Z", false, BERLIN).unwrap(),
            "FREQ=DAILY;UNTIL=20261130T090000Z"
        );
        assert_eq!(
            normalize("FREQ=DAILY;UNTIL=20261130", true, BERLIN).unwrap(),
            "FREQ=DAILY;UNTIL=20261130T235959Z"
        );
        assert!(normalize("FREQ", false, BERLIN).is_err());
    }

    #[test]
    fn keeps_wall_clock_across_dst() {
        // Berlin leaves summer time on 2026-10-25.
        let ev = weekly("FREQ=WEEKLY");
        let starts = starts_between(&ev, at(2026, 10, 1, 0, 0), at(2026, 11, 3, 0, 0)).unwrap();
        let hours: Vec<_> = starts
            .iter()
            .map(|&s| time::in_tz(s, BERLIN).format("%m-%d %H:%M").to_string())
            .collect();
        assert_eq!(hours, ["10-19 09:00", "10-26 09:00", "11-02 09:00"]);
        assert_eq!(starts[1] - starts[0], 7 * time::DAY + 3600);
    }

    #[test]
    fn skipped_time_moves_past_the_gap() {
        // São Paulo skipped from 00:00 to 01:00 on 2007-10-14.
        let sp = chrono_tz::America::Sao_Paulo;
        let midnight = |d| time::local_midnight(NaiveDate::from_ymd_opt(2007, 10, d).unwrap(), sp);
        let mut ev = Event::new("x", midnight(12), midnight(12) + 1800);
        ev.tzid = Some("America/Sao_Paulo".into());
        ev.rrule = Some("FREQ=DAILY;COUNT=4".into());
        let starts = starts_between(&ev, midnight(1), midnight(20)).unwrap();
        let shown: Vec<_> = starts
            .iter()
            .map(|&s| time::in_tz(s, sp).format("%d %H:%M").to_string())
            .collect();
        assert_eq!(shown, ["12 00:00", "13 00:00", "14 01:00", "15 00:00"]);
        assert_eq!(series_end(&ev).unwrap(), Some(starts[3] + 1800));
    }

    #[test]
    fn bounded_series_end() {
        let ev = weekly("FREQ=WEEKLY;COUNT=3");
        assert_eq!(series_end(&ev).unwrap(), Some(at(2026, 11, 2, 9, 30)));
        let ev = weekly("FREQ=WEEKLY;UNTIL=20261028");
        assert_eq!(series_end(&ev).unwrap(), Some(at(2026, 10, 26, 9, 30)));
        assert_eq!(series_end(&weekly("FREQ=WEEKLY")).unwrap(), None);
    }

    #[test]
    fn window_is_half_open() {
        let ev = weekly("FREQ=WEEKLY");
        let monday = at(2026, 10, 26, 9, 0);
        assert_eq!(starts_between(&ev, monday, monday + 1).unwrap(), [monday]);
        assert!(starts_between(&ev, monday - 1, monday).unwrap().is_empty());
    }

    #[test]
    fn rejects_garbage() {
        let mut ev = weekly("FREQ=WEEKLY");
        ev.rrule = Some("FREQ=SOMETIMES".into());
        assert!(rule_set(&ev).is_err());
    }
}
