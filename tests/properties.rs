//! Property tests: the parsers never panic on any input, export → import
//! keeps events as they were, and repeating events keep their wall-clock
//! time across DST changes in any zone.

use chrono::{NaiveDate, TimeZone as _, Timelike};
use chrono_tz::Tz;
use proptest::prelude::*;
use vcal::db::Db;
use vcal::ics;
use vcal::model::{Event, Exception, Status};
use vcal::time::{self, DAY};

/// Zones with DST in both hemispheres, odd offsets, and none at all.
const ZONES: &[&str] = &[
    "Europe/Berlin",
    "America/New_York",
    "America/Sao_Paulo",
    "Australia/Sydney",
    "Australia/Lord_Howe",
    "Asia/Kolkata",
    "Asia/Kathmandu",
    "Pacific/Chatham",
    "Africa/Casablanca",
    "UTC",
];

fn zone() -> impl Strategy<Value = Tz> {
    prop::sample::select(ZONES).prop_map(|z| z.parse().unwrap())
}

/// Text with the characters iCalendar has to escape, and some that aren't ASCII.
fn text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ;,:\\\\\nüé日本🎉\"']{1,90}".prop_map(|s| s.trim().to_string())
}

/// Local wall-clock times between 2000 and 2040, at whole minutes.
fn local_start() -> impl Strategy<Value = (NaiveDate, u32, u32)> {
    (0i64..14_600, 0u32..24, 0u32..60).prop_map(|(day, h, m)| {
        (
            NaiveDate::from_ymd_opt(2000, 1, 1).unwrap() + chrono::Duration::days(day),
            h,
            m,
        )
    })
}

fn rule() -> impl Strategy<Value = Option<String>> {
    prop::sample::select(vec![
        None,
        Some("FREQ=DAILY;COUNT=40"),
        Some("FREQ=WEEKLY;BYDAY=MO,WE,FR"),
        Some("FREQ=WEEKLY;INTERVAL=2;COUNT=30"),
        Some("FREQ=MONTHLY;BYMONTHDAY=15"),
        Some("FREQ=YEARLY"),
    ])
    .prop_map(|r| r.map(str::to_string))
}

prop_compose! {
    fn event()(
        title in text(),
        notes in prop::option::of(text()),
        location in prop::option::of(text()),
        category in prop::option::of("[a-z]{1,10}"),
        tz in zone(),
        (date, h, m) in local_start(),
        minutes in 0i64..3 * 24 * 60,
        all_day in any::<bool>(),
        days in 1i64..5,
        rrule in rule(),
        reminder in prop::option::of(0i64..10_000),
        tentative in any::<bool>(),
    ) -> Event {
        let mut ev = if all_day {
            let s = time::floating(date);
            let mut ev = Event::new(title.clone(), s, s + days * DAY);
            ev.all_day = true;
            ev
        } else {
            let s = time::resolve_local(date.and_hms_opt(h, m, 0).unwrap(), tz);
            let mut ev = Event::new(title.clone(), s, s + minutes * 60);
            ev.tzid = Some(tz.name().to_string());
            ev
        };
        if ev.title.is_empty() {
            ev.title = "x".into();
        }
        ev.description = notes.unwrap_or_default();
        ev.location = location.filter(|l| !l.is_empty());
        ev.category = category;
        ev.rrule = rrule;
        ev.reminder = reminder;
        if tentative {
            ev.status = Status::Tentative;
        }
        ev
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    #[test]
    fn parsers_never_panic(input in ".{0,400}") {
        let _ = ics::parse(&input, Tz::UTC);
        let _ = ics::parse(&format!("BEGIN:VCALENDAR\r\n{input}\r\nEND:VCALENDAR"), Tz::UTC);
        let _ = time::parse_when(&input, Tz::UTC, 1_800_000_000);
        let _ = time::parse_duration(&input);
        let _ = time::parse_reminder(&input);
        let today = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let _ = vcal::quick::parse(&input, today, today, Tz::UTC, 3600);
    }

    /// A real file, cut and spliced at random places, is still handled.
    #[test]
    fn damaged_files_dont_panic(ev in event(), cut in any::<prop::sample::Index>(), junk in ".{0,40}") {
        let text = ics::write::calendar(&[(ev, vec![])], 1_800_000_000);
        let at = cut.index(text.len());
        let at = (0..=at).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
        let damaged = format!("{}{junk}{}", &text[..at], &text[at..]);
        let _ = ics::parse(&damaged, Tz::UTC);
        let _ = ics::parse(&text[..at], Tz::UTC);
    }

    #[test]
    fn export_then_import_keeps_events(ev in event(), local in zone()) {
        let mut db = Db::open_in_memory().unwrap();
        let saved = db.insert(ev).unwrap();
        let mut exceptions = vec![];
        if saved.is_recurring() {
            let occ: Vec<i64> = vcal::db::recurrence::starts_between(&saved, saved.start, saved.start + 400 * DAY).unwrap();
            if occ.len() > 2 {
                db.set_exception(Exception::cancelled(&saved.id, occ[1])).unwrap();
                let mut moved = Exception::modified(&saved.id, occ[2]);
                moved.title = Some("moved".into());
                let by = if saved.all_day { DAY } else { 3600 };
                moved.start = Some(occ[2] + by);
                moved.end = Some(occ[2] + by + saved.duration());
                db.set_exception(moved).unwrap();
            }
            exceptions = db.exceptions(&saved.id).unwrap();
        }
        let saved = db.get(&saved.id).unwrap().unwrap();
        let text = ics::write::calendar(&[(saved.clone(), exceptions.clone())], 1_800_000_000);
        for line in text.split("\r\n") {
            prop_assert!(line.len() <= 75, "a line over 75 octets: {line:?}");
        }
        // read in a different local zone: nothing may depend on it
        let p = ics::parse(&text, local).unwrap();
        prop_assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        prop_assert_eq!(p.events.len(), 1);
        let got = &p.events[0];
        prop_assert_eq!(&got.id, &saved.id);
        prop_assert_eq!(&got.title, &saved.title);
        prop_assert_eq!(&got.description, &saved.description);
        prop_assert_eq!(&got.location, &saved.location);
        prop_assert_eq!(&got.category, &saved.category);
        prop_assert_eq!((got.start, got.end, got.all_day), (saved.start, saved.end, saved.all_day));
        prop_assert_eq!(&got.rrule, &saved.rrule);
        prop_assert_eq!(got.reminder, saved.reminder);
        prop_assert_eq!(got.status, saved.status);
        if !saved.all_day {
            prop_assert_eq!(&got.tzid, &saved.tzid);
        }
        prop_assert_eq!(p.exceptions.len(), exceptions.len());
        for (a, b) in p.exceptions.iter().zip(&exceptions) {
            prop_assert_eq!((a.original_start, a.kind), (b.original_start, b.kind));
            if b.start.is_some() {
                prop_assert_eq!((a.start, a.end), (b.start, b.end));
            }
        }
    }

    /// Every occurrence of a daily or weekly series starts at the same
    /// local time as the first (unless DST skipped that time that day), and
    /// the number of occurrences matches COUNT.
    #[test]
    fn series_keep_their_wall_clock(tz in zone(), (date, h, m) in local_start(), weekly in any::<bool>(), count in 2u32..120) {
        let start = time::resolve_local(date.and_hms_opt(h, m, 0).unwrap(), tz);
        let mut ev = Event::new("x", start, start + 1800);
        ev.tzid = Some(tz.name().to_string());
        ev.rrule = Some(format!("FREQ={};COUNT={count}", if weekly { "WEEKLY" } else { "DAILY" }));
        let mut db = Db::open_in_memory().unwrap();
        let ev = db.insert(ev).unwrap();
        let first = time::in_tz(ev.start, tz);
        let occ = db.occurrences(ev.start, ev.start + 900 * DAY, tz, false).unwrap();
        prop_assert_eq!(occ.len() as u32, count);
        for o in &occ {
            let t = time::in_tz(o.start, tz);
            let same = (t.hour(), t.minute()) == (first.hour(), first.minute());
            // a skipped local time moves past the gap; it must still be that day
            let skipped = !same
                && tz
                    .from_local_datetime(&t.date_naive().and_hms_opt(first.hour(), first.minute(), 0).unwrap())
                    .single()
                    .is_none();
            prop_assert!(same || skipped, "{} in {tz}: {} isn't {}", o.start, t, first);
            prop_assert_eq!(o.end - o.start, 1800);
        }
        let days: Vec<i64> = occ.windows(2).map(|w| (time::local_date(w[1].start, tz) - time::local_date(w[0].start, tz)).num_days()).collect();
        prop_assert!(days.iter().all(|d| *d == if weekly { 7 } else { 1 }), "{days:?}");
    }
}
