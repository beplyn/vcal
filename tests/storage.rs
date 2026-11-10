use chrono::NaiveDate;
use chrono_tz::Tz;
use vcal::db::{self, Db};
use vcal::model::{Event, Exception, Status};
use vcal::time::{self, DAY};

const BERLIN: Tz = chrono_tz::Europe::Berlin;
const LA: Tz = chrono_tz::America::Los_Angeles;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at(tz: Tz, y: i32, m: u32, d: u32, h: u32) -> i64 {
    time::resolve_local(date(y, m, d).and_hms_opt(h, 0, 0).unwrap(), tz)
}

fn timed(title: &str, start: i64, hours: i64) -> Event {
    let mut ev = Event::new(title, start, start + hours * 3600);
    ev.tzid = Some("Europe/Berlin".into());
    ev
}

fn all_day(title: &str, first: NaiveDate, days: i64) -> Event {
    let mut ev = Event::new(
        title,
        time::floating(first),
        time::floating(first) + days * DAY,
    );
    ev.all_day = true;
    ev
}

fn day_titles(db: &Db, d: NaiveDate, tz: Tz) -> Vec<String> {
    let from = time::local_midnight(d, tz);
    let to = time::local_midnight(d.succ_opt().unwrap(), tz);
    db.occurrences(from, to, tz, false)
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect()
}

#[test]
fn opens_in_wal_and_migrates_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/events.db");
    let db = Db::open(&path).unwrap();
    let mode: String = db
        .conn()
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    let fk: i64 = db
        .conn()
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    drop(db);

    // Re-opening doesn't re-run migrations (they'd fail on CREATE TABLE).
    let db = Db::open(&path).unwrap();
    let version: i64 = db
        .conn()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, db::schema_version());
    let indexes: Vec<String> = db
        .conn()
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(indexes.contains(&"idx_events_range".to_string()));
    assert!(indexes.contains(&"idx_events_allday".to_string()));
}

#[test]
fn refuses_a_newer_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let db = Db::open(&path).unwrap();
    db.conn().pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    let err = Db::open(&path).err().unwrap();
    assert!(format!("{err:#}").contains("newer"), "{err:#}");
}

#[test]
fn crud_round_trip() {
    let mut db = Db::open_in_memory().unwrap();
    let mut ev = timed("  Dentist  ", at(BERLIN, 2026, 11, 10, 14), 1);
    ev.category = Some(" ".into());
    let saved = db.insert(ev).unwrap();
    assert_eq!(saved.title, "Dentist");
    assert_eq!(saved.category, None);
    assert_eq!(db.get(&saved.id).unwrap().unwrap(), saved);

    let mut changed = saved.clone();
    changed.title = "Dentist (moved)".into();
    changed.start += 3600;
    changed.end += 3600;
    db.update(changed).unwrap();
    let got = db.get(&saved.id).unwrap().unwrap();
    assert_eq!(got.title, "Dentist (moved)");
    assert_eq!(got.created, saved.created);

    assert!(db.delete(&saved.id).unwrap());
    assert!(!db.delete(&saved.id).unwrap());
    assert!(db.get(&saved.id).unwrap().is_none());
    assert!(db.update(saved).is_err());
}

#[test]
fn rejects_bad_events() {
    let mut db = Db::open_in_memory().unwrap();
    let start = at(BERLIN, 2026, 11, 10, 9);
    assert!(db.insert(timed("", start, 1)).is_err());
    assert!(
        db.insert(Event::new("backwards", start, start - 1))
            .is_err()
    );
    let mut ev = timed("bad rule", start, 1);
    ev.rrule = Some("FREQ=FORTNIGHTLY".into());
    assert!(db.insert(ev).is_err());
    let mut ev = all_day("half day", date(2026, 11, 10), 1);
    ev.end -= 3600;
    assert!(db.insert(ev).is_err());
}

#[test]
fn range_query_overlap_rules() {
    let mut db = Db::open_in_memory().unwrap();
    let nine = at(BERLIN, 2026, 11, 10, 9);
    db.insert(timed("morning", nine, 1)).unwrap();
    db.insert(timed("overnight", at(BERLIN, 2026, 11, 9, 22), 4))
        .unwrap();
    db.insert(timed("reminder", nine + 1800, 0)).unwrap();
    // A long event must still be found even though it starts far before the window.
    db.insert(timed("conference", at(BERLIN, 2026, 11, 1, 9), 24 * 12))
        .unwrap();
    db.insert(timed("next day", at(BERLIN, 2026, 11, 11, 0), 1))
        .unwrap();

    let titles = day_titles(&db, date(2026, 11, 10), BERLIN);
    assert_eq!(titles, ["conference", "overnight", "morning", "reminder"]);

    // Window ends exactly when "morning" starts: excluded.
    let before: Vec<_> = db
        .occurrences(nine - 3600, nine, BERLIN, false)
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect();
    assert_eq!(before, ["conference"]);
}

#[test]
fn all_day_events_follow_the_viewers_dates() {
    let mut db = Db::open_in_memory().unwrap();
    db.insert(all_day("holiday", date(2026, 11, 10), 1))
        .unwrap();
    db.insert(all_day("trip", date(2026, 11, 11), 3)).unwrap();
    for tz in [BERLIN, LA, Tz::UTC, chrono_tz::Pacific::Kiritimati] {
        assert_eq!(
            day_titles(&db, date(2026, 11, 9), tz),
            Vec::<String>::new(),
            "{tz}"
        );
        assert_eq!(day_titles(&db, date(2026, 11, 10), tz), ["holiday"], "{tz}");
        assert_eq!(day_titles(&db, date(2026, 11, 13), tz), ["trip"], "{tz}");
        assert_eq!(
            day_titles(&db, date(2026, 11, 14), tz),
            Vec::<String>::new(),
            "{tz}"
        );
    }
}

#[test]
fn sorts_all_day_first() {
    let mut db = Db::open_in_memory().unwrap();
    db.insert(timed("early", at(BERLIN, 2026, 11, 10, 0), 1))
        .unwrap();
    db.insert(all_day("birthday", date(2026, 11, 10), 1))
        .unwrap();
    assert_eq!(
        day_titles(&db, date(2026, 11, 10), BERLIN),
        ["birthday", "early"]
    );
}

#[test]
fn recurring_series_with_exceptions() {
    let mut db = Db::open_in_memory().unwrap();
    let mut ev = timed("standup", at(BERLIN, 2026, 10, 19, 9), 1);
    ev.rrule = Some("FREQ=WEEKLY;BYDAY=MO".into());
    let ev = db.insert(ev).unwrap();

    let oct26 = at(BERLIN, 2026, 10, 26, 9);
    let nov2 = at(BERLIN, 2026, 11, 2, 9);
    let nov9 = at(BERLIN, 2026, 11, 9, 9);
    db.cancel_occurrence(&ev.id, oct26).unwrap();
    let mut late = Exception::modified(&ev.id, nov2);
    late.title = Some("late standup".into());
    late.start = Some(nov2 + 3600);
    db.set_exception(late).unwrap();
    // Moved from Monday the 9th to Saturday the 14th.
    let mut moved = Exception::modified(&ev.id, nov9);
    moved.start = Some(at(BERLIN, 2026, 11, 14, 11));
    db.set_exception(moved).unwrap();

    let from = at(BERLIN, 2026, 10, 19, 0);
    let to = at(BERLIN, 2026, 11, 17, 0);
    let occ = db.occurrences(from, to, BERLIN, false).unwrap();
    let got: Vec<_> = occ
        .iter()
        .map(|o| {
            (
                time::in_tz(o.start, BERLIN)
                    .format("%m-%d %H:%M")
                    .to_string(),
                o.title.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("10-19 09:00".into(), "standup"),
            ("11-02 10:00".into(), "late standup"),
            ("11-14 11:00".into(), "standup"),
            ("11-16 09:00".into(), "standup"),
        ]
    );
    assert_eq!(
        occ[1].end - occ[1].start,
        3600,
        "moved occurrence keeps the series length"
    );
    assert!(occ[1].modified && !occ[0].modified);
    assert_eq!(occ[2].original_start, Some(nov9));

    // The moved occurrence shows on its new day only, even queried alone.
    assert_eq!(day_titles(&db, date(2026, 11, 14), BERLIN), ["standup"]);
    assert!(day_titles(&db, date(2026, 11, 9), BERLIN).is_empty());

    // Exceptions go with the series.
    assert_eq!(db.exceptions(&ev.id).unwrap().len(), 3);
    db.delete(&ev.id).unwrap();
    let left: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM event_exceptions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn bounded_series_stop() {
    let mut db = Db::open_in_memory().unwrap();
    let mut ev = all_day("course", date(2026, 11, 2), 1);
    ev.rrule = Some("FREQ=DAILY;COUNT=3".into());
    db.insert(ev).unwrap();
    assert_eq!(day_titles(&db, date(2026, 11, 4), LA), ["course"]);
    assert!(day_titles(&db, date(2026, 11, 5), LA).is_empty());
    let series_end: i64 = db
        .conn()
        .query_row("SELECT series_end_ts FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(series_end, time::floating(date(2026, 11, 5)));
}

#[test]
fn cancelled_events_are_opt_in() {
    let mut db = Db::open_in_memory().unwrap();
    let mut ev = timed("called off", at(BERLIN, 2026, 11, 10, 9), 1);
    ev.status = Status::Cancelled;
    db.insert(ev).unwrap();
    let (from, to) = (at(BERLIN, 2026, 11, 10, 0), at(BERLIN, 2026, 11, 11, 0));
    assert!(db.occurrences(from, to, BERLIN, false).unwrap().is_empty());
    assert_eq!(db.occurrences(from, to, BERLIN, true).unwrap().len(), 1);
}

#[test]
fn exceptions_need_a_series() {
    let mut db = Db::open_in_memory().unwrap();
    let single = db
        .insert(timed("once", at(BERLIN, 2026, 11, 10, 9), 1))
        .unwrap();
    assert!(db.cancel_occurrence(&single.id, single.start).is_err());
    assert!(db.cancel_occurrence("nope", 0).is_err());
}

#[test]
fn writes_touch_the_sentinel() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let db = Db::open(&path).unwrap();
    let sentinel = db::sentinel_path(&path);
    assert_eq!(sentinel.file_name().unwrap(), "events.db.changed");
    // opening creates it empty, so watchers have something to follow
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "");
    drop(db);
    let mut db = Db::open(&path).unwrap();
    assert_eq!(
        std::fs::read_to_string(&sentinel).unwrap(),
        "",
        "re-opening isn't a change"
    );
    db.insert(timed("x", at(BERLIN, 2026, 11, 10, 9), 1))
        .unwrap();
    let first = std::fs::read_to_string(&sentinel).unwrap();
    db.insert(timed("y", at(BERLIN, 2026, 11, 10, 9), 1))
        .unwrap();
    assert_ne!(std::fs::read_to_string(&sentinel).unwrap(), first);
}

#[test]
fn readers_and_writers_dont_block_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let mut writer = Db::open(&path).unwrap();
    let reader = Db::open(&path).unwrap();
    let start = at(BERLIN, 2026, 11, 10, 9);
    writer.insert(timed("before", start, 1)).unwrap();

    // An open read transaction holds a snapshot...
    reader.conn().execute_batch("BEGIN").unwrap();
    let count = |db: &Db| -> i64 {
        db.conn()
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(count(&reader), 1);
    // ...while the writer commits without waiting for it.
    let t = std::time::Instant::now();
    writer.insert(timed("during", start, 1)).unwrap();
    assert!(t.elapsed().as_millis() < 1000, "writer waited on reader");
    assert_eq!(count(&reader), 1, "reader keeps its snapshot");
    reader.conn().execute_batch("COMMIT").unwrap();
    assert_eq!(count(&reader), 2);

    // Concurrent writers from threads serialise through the busy timeout.
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut db = Db::open(&path).unwrap();
                for j in 0..10 {
                    db.insert(timed(&format!("t{i}-{j}"), start, 1)).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(count(&reader), 42);
}

#[test]
fn import_is_idempotent() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:weekly@x\nSUMMARY:Weekly\n\
DTSTART;TZID=Europe/Berlin:20261019T090000\nDTEND;TZID=Europe/Berlin:20261019T100000\n\
RRULE:FREQ=WEEKLY\nEXDATE;TZID=Europe/Berlin:20261026T090000\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:orphan@x\nRECURRENCE-ID:20261026T080000Z\nDTSTART:20261026T080000Z\n\
SUMMARY:no parent\nEND:VEVENT\nEND:VCALENDAR\n";
    let mut db = Db::open_in_memory().unwrap();
    for _ in 0..2 {
        let parsed = vcal::ics::parse(ics, BERLIN).unwrap();
        assert_eq!(db.import(parsed.events, parsed.exceptions).unwrap(), (1, 1));
    }
    let n: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(day_titles(&db, date(2026, 10, 26), BERLIN).is_empty());
    assert_eq!(day_titles(&db, date(2026, 11, 2), BERLIN), ["Weekly"]);
}

#[test]
fn dropping_the_rule_drops_exceptions() {
    let mut db = Db::open_in_memory().unwrap();
    let mut ev = timed("standup", at(BERLIN, 2026, 11, 9, 9), 1);
    ev.rrule = Some("FREQ=DAILY".into());
    let ev = db.insert(ev).unwrap();
    let mut moved = Exception::modified(&ev.id, at(BERLIN, 2026, 11, 10, 9));
    moved.start = Some(at(BERLIN, 2026, 11, 12, 15));
    db.set_exception(moved).unwrap();

    let mut single = ev.clone();
    single.rrule = None;
    db.update(single).unwrap();
    assert!(db.exceptions(&ev.id).unwrap().is_empty());
    assert!(day_titles(&db, date(2026, 11, 12), BERLIN).is_empty());
    assert_eq!(day_titles(&db, date(2026, 11, 9), BERLIN), ["standup"]);
}

#[test]
fn reimport_follows_the_feed() {
    let feed = |exdate: &str| {
        format!(
            "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:weekly@x\nSUMMARY:Weekly\n\
DTSTART;TZID=Europe/Berlin:20261019T090000\nDTEND;TZID=Europe/Berlin:20261019T100000\n\
RRULE:FREQ=WEEKLY\n{exdate}BEGIN:VALARM\nTRIGGER:-PT10M\nEND:VALARM\nEND:VEVENT\nEND:VCALENDAR\n"
        )
    };
    let mut db = Db::open_in_memory().unwrap();
    let import = |db: &mut Db, text: &str| {
        let parsed = vcal::ics::parse(text, BERLIN).unwrap();
        db.import(parsed.events, parsed.exceptions).unwrap()
    };
    import(
        &mut db,
        &feed("EXDATE;TZID=Europe/Berlin:20261026T090000\n"),
    );
    assert!(day_titles(&db, date(2026, 10, 26), BERLIN).is_empty());
    let created = db.get("weekly@x").unwrap().unwrap().created;

    // the organiser un-cancelled it
    import(&mut db, &feed(""));
    assert_eq!(day_titles(&db, date(2026, 10, 26), BERLIN), ["Weekly"]);
    let ev = db.get("weekly@x").unwrap().unwrap();
    assert_eq!(ev.reminder, Some(10));
    assert_eq!(ev.created, created, "an update, not a new event");
}

#[test]
fn reminders_and_notified_marks() {
    let mut db = Db::open_in_memory().unwrap();
    let start = at(BERLIN, 2026, 11, 9, 9);
    let mut ev = timed("standup", start, 1);
    ev.reminder = Some(15);
    let ev = db.insert(ev).unwrap();
    assert_eq!(db.get(&ev.id).unwrap().unwrap().reminder, Some(15));
    assert_eq!(db.max_reminder().unwrap(), 15);
    let occ = &db.occurrences(start, start + 1, BERLIN, false).unwrap()[0];
    assert_eq!(occ.reminder, Some(15));

    let mut bad = ev.clone();
    bad.reminder = Some(-5);
    assert!(db.update(bad).is_err());

    assert!(!db.was_notified(&ev.id, start).unwrap());
    db.mark_notified(&[(ev.id.clone(), start)], start, 0)
        .unwrap();
    assert!(db.was_notified(&ev.id, start).unwrap());
    // old marks are forgotten
    db.mark_notified(&[], start, start + 1).unwrap();
    assert!(!db.was_notified(&ev.id, start).unwrap());
    // and go with their event
    db.mark_notified(&[(ev.id.clone(), start)], start, 0)
        .unwrap();
    db.delete(&ev.id).unwrap();
    let n: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM notified", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
}
