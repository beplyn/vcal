//! RRULE expansion. Timed series expand in their own zone so they keep their
//! wall-clock time across DST; all-day series expand in UTC over floating dates.

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

/// The expanded rule set, or `None` for a single event.
pub fn rule_set(ev: &Event) -> Result<Option<RRuleSet>> {
    let Some(rule) = ev.rrule.as_deref() else {
        return Ok(None);
    };
    let tz = zone(ev);
    let parsed: RRule<Unvalidated> = rule
        .parse()
        .with_context(|| format!("invalid RRULE {rule:?}"))?;
    let start = tz
        .timestamp_opt(ev.start, 0)
        .single()
        .ok_or_else(|| anyhow!("start {} out of range", ev.start))?
        .with_timezone(&rrule::Tz::Tz(tz));
    let set = parsed
        .build(start)
        .with_context(|| format!("invalid RRULE {rule:?}"))?;
    Ok(Some(set))
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
    let last = result.dates.last().map_or(ev.start, |d| d.timestamp());
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
    let tz = rrule::Tz::Tz(zone(ev));
    let at = |ts: i64| {
        tz.timestamp_opt(ts, 0)
            .single()
            .ok_or_else(|| anyhow!("{ts} out of range"))
    };
    let dates = set
        .after(at(lo)?)
        .before(at(hi)?)
        .all(MAX_OCCURRENCES)
        .dates;
    Ok(dates
        .iter()
        .map(|d| d.timestamp())
        .filter(|&s| s < hi)
        .collect())
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
