//! Instants, floating dates and the user-facing `<when>` / duration syntax.
//!
//! Timed events are UTC epoch seconds. All-day events are *floating*: the
//! calendar date stored as UTC midnight, never shifted by a time zone.
//! Time zones are always `chrono_tz` (embedded data), never `chrono::Local`,
//! so behaviour does not depend on /usr/share/zoneinfo being present.

use anyhow::{Context, Result, bail};
use chrono::{
    DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset, TimeZone, Utc,
};
use chrono_tz::Tz;

pub const DAY: i64 = 86_400;

/// The user's zone: `$TZ` if it names an IANA zone, else the system's, else UTC.
pub fn local_tz() -> Tz {
    let from_env = std::env::var("TZ")
        .ok()
        .and_then(|v| v.trim_start_matches(':').parse::<Tz>().ok());
    from_env
        .or_else(|| iana_time_zone::get_timezone().ok()?.parse().ok())
        .unwrap_or(Tz::UTC)
}

pub fn now() -> i64 {
    Utc::now().timestamp()
}

/// Wall-clock time in `tz` to an instant. Ambiguous times (DST fall-back)
/// take the earlier instant; skipped times (spring-forward) move past the gap.
pub fn resolve_local(naive: NaiveDateTime, tz: Tz) -> i64 {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.timestamp(),
        LocalResult::Ambiguous(early, _) => early.timestamp(),
        LocalResult::None => {
            // No gap is longer than a few hours; step forward until valid.
            (1..=24)
                .find_map(|m| {
                    tz.from_local_datetime(&(naive + Duration::minutes(15 * m)))
                        .earliest()
                })
                .map(|dt| dt.timestamp())
                .unwrap_or_else(|| naive.and_utc().timestamp())
        }
    }
}

pub fn local_midnight(date: NaiveDate, tz: Tz) -> i64 {
    resolve_local(date.and_time(NaiveTime::MIN), tz)
}

/// A floating date as stored for all-day events.
pub fn floating(date: NaiveDate) -> i64 {
    date.and_time(NaiveTime::MIN).and_utc().timestamp()
}

pub fn floating_date(ts: i64) -> NaiveDate {
    datetime_utc(ts).date_naive()
}

/// The floating timestamp with the same wall-clock reading as `ts` in `tz`,
/// used to compare instants against all-day events.
pub fn to_floating(ts: i64, tz: Tz) -> i64 {
    let offset = tz
        .offset_from_utc_datetime(&datetime_utc(ts).naive_utc())
        .fix();
    ts + i64::from(offset.local_minus_utc())
}

pub fn datetime_utc(ts: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(ts, 0).unwrap_or_default()
}

pub fn in_tz(ts: i64, tz: Tz) -> DateTime<Tz> {
    datetime_utc(ts).with_timezone(&tz)
}

pub fn local_date(ts: i64, tz: Tz) -> NaiveDate {
    in_tz(ts, tz).date_naive()
}

/// A parsed `<when>`: either a concrete instant or a bare calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Instant(i64),
    Date(NaiveDate),
}

impl When {
    /// Instant; bare dates mean local midnight.
    pub fn instant(self, tz: Tz) -> i64 {
        match self {
            When::Instant(ts) => ts,
            When::Date(d) => local_midnight(d, tz),
        }
    }

    pub fn date(self, tz: Tz) -> NaiveDate {
        match self {
            When::Instant(ts) => local_date(ts, tz),
            When::Date(d) => d,
        }
    }
}

/// Parses `<when>`:
/// `now`, `today`, `tomorrow`, `yesterday` (optionally followed by `HH:MM`),
/// `+3d` / `-2h` / `+1w` / `+30m` relative to now, epoch seconds, RFC 3339,
/// `YYYY-MM-DD`, or `YYYY-MM-DD HH:MM[:SS]` (also with `T`) in local time.
pub fn parse_when(input: &str, tz: Tz, now: i64) -> Result<When> {
    let s = input.trim();
    let today = local_date(now, tz);

    let (head, time) = match s.split_once(char::is_whitespace) {
        Some((h, t)) if parse_time(t.trim()).is_some() => (h, parse_time(t.trim())),
        _ => (s, None),
    };
    let word = match head.to_ascii_lowercase().as_str() {
        "now" if time.is_none() => return Ok(When::Instant(now)),
        "today" => Some(today),
        "tomorrow" => today.succ_opt(),
        "yesterday" => today.pred_opt(),
        _ => None,
    };
    if let Some(date) = word {
        return Ok(match time {
            Some(t) => When::Instant(resolve_local(date.and_time(t), tz)),
            None => When::Date(date),
        });
    }

    if let Some(rest) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) {
        let sign = if s.starts_with('-') { -1 } else { 1 };
        let secs = parse_duration(rest).with_context(|| format!("bad relative time {s:?}"))?;
        return Ok(When::Instant(now + sign * secs));
    }
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.len() > 8 {
        return Ok(When::Instant(s.parse()?));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(When::Instant(dt.timestamp()));
    }
    for fmt in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(When::Instant(resolve_local(naive, tz)));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(When::Date(date));
    }
    bail!("can't read {input:?} as a time (try 2026-11-10, \"2026-11-10 14:00\", tomorrow, +2h)")
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M:%S"))
        .ok()
}

/// Parses `90m`, `1h30m`, `2d`, `1w`, `45s`; a bare number is minutes.
pub fn parse_duration(input: &str) -> Result<i64> {
    let s = input.trim();
    if s.is_empty() {
        bail!("empty duration");
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(s.parse::<i64>()? * 60);
    }
    let mut total = 0i64;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => DAY,
            'w' => 7 * DAY,
            _ => bail!("bad duration {input:?}: unknown unit {c:?}"),
        };
        if num.is_empty() {
            bail!("bad duration {input:?}");
        }
        total += num.parse::<i64>()? * unit;
        num.clear();
    }
    if !num.is_empty() {
        bail!("bad duration {input:?}: trailing number needs a unit");
    }
    Ok(total)
}

/// A reminder lead time such as `10m` or `1d`, in whole minutes; empty
/// means no reminder.
pub fn parse_reminder(input: &str) -> Result<Option<i64>> {
    if input.trim().is_empty() {
        return Ok(None);
    }
    let secs = parse_duration(input)?;
    if secs % 60 != 0 {
        bail!("reminders are whole minutes, not {input:?}");
    }
    Ok(Some(secs / 60))
}

/// Minutes as the shortest `parse_duration` text: `0m`, `45m`, `1h30m`, `2d`.
pub fn format_minutes(minutes: i64) -> String {
    if minutes == 0 {
        return "0m".into();
    }
    let mut out = String::new();
    let mut left = minutes;
    for (unit, size) in [("d", 24 * 60), ("h", 60), ("m", 1)] {
        if left >= size {
            out.push_str(&format!("{}{unit}", left / size));
            left %= size;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders() {
        assert_eq!(parse_reminder("").unwrap(), None);
        assert_eq!(parse_reminder("10").unwrap(), Some(10));
        assert_eq!(parse_reminder("1h30m").unwrap(), Some(90));
        assert!(parse_reminder("30s").is_err());
        for m in [0, 5, 60, 90, 1440, 1505] {
            assert_eq!(parse_reminder(&format_minutes(m)).unwrap(), Some(m));
        }
        assert_eq!(format_minutes(1505), "1d1h5m");
    }

    const BERLIN: Tz = chrono_tz::Europe::Berlin;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90").unwrap(), 5400);
        assert_eq!(parse_duration("1h30m").unwrap(), 5400);
        assert_eq!(parse_duration("2d").unwrap(), 2 * DAY);
        assert!(parse_duration("3x").is_err());
        assert!(parse_duration("1h30").is_err());
    }

    #[test]
    fn when_forms() {
        let now = resolve_local(ymd(2026, 11, 9).and_hms_opt(10, 0, 0).unwrap(), BERLIN);
        let p = |s| parse_when(s, BERLIN, now).unwrap();
        assert_eq!(p("today"), When::Date(ymd(2026, 11, 9)));
        assert_eq!(p("tomorrow"), When::Date(ymd(2026, 11, 10)));
        assert_eq!(p("2026-12-01"), When::Date(ymd(2026, 12, 1)));
        assert_eq!(p("+2h"), When::Instant(now + 7200));
        assert_eq!(p("-1d"), When::Instant(now - DAY));
        assert_eq!(p("now"), When::Instant(now));
        assert_eq!(p("1700000000"), When::Instant(1_700_000_000));
        assert_eq!(p("2026-11-09T09:00:00Z"), When::Instant(now));
        // CET is UTC+1 in November
        assert_eq!(p("2026-11-09 11:00"), When::Instant(now + 3600));
        assert_eq!(p("tomorrow 10:00"), When::Instant(now + DAY));
        assert!(parse_when("next blursday", BERLIN, now).is_err());
    }

    #[test]
    fn dst_edges() {
        // 2026-03-29 02:30 doesn't exist in Berlin; it lands after the gap at 03:00 CEST.
        let gap = resolve_local(ymd(2026, 3, 29).and_hms_opt(2, 30, 0).unwrap(), BERLIN);
        assert_eq!(in_tz(gap, BERLIN).format("%H:%M").to_string(), "03:00");
        // 2026-10-25 02:30 happens twice; take the first (CEST, UTC+2).
        let amb = resolve_local(ymd(2026, 10, 25).and_hms_opt(2, 30, 0).unwrap(), BERLIN);
        assert_eq!(in_tz(amb, BERLIN).format("%z").to_string(), "+0200");
    }

    #[test]
    fn floating_round_trip() {
        let d = ymd(2026, 11, 10);
        assert_eq!(floating_date(floating(d)), d);
        // 23:30 in Berlin on the 10th is still the 10th as a floating time
        let late = resolve_local(d.and_hms_opt(23, 30, 0).unwrap(), BERLIN);
        assert_eq!(floating_date(to_floating(late, BERLIN)), d);
    }
}
