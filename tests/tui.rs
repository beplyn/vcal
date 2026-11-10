//! The TUI driven headlessly: key sequences in Vim notation go through the
//! real keymap and update loop, and frames render into a TestBackend.

use chrono::NaiveDate;
use chrono_tz::Tz;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use vcal::db::Db;
use vcal::model::Event;
use vcal::settings::{Settings, View};
use vcal::time::{self, DAY};
use vcal::tui::App;
use vcal::tui::app::{Focus, Mode};
use vcal::tui::keymap::parse_keys;

const BERLIN: Tz = chrono_tz::Europe::Berlin;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at(d: NaiveDate, h: u32, m: u32) -> i64 {
    time::resolve_local(d.and_hms_opt(h, m, 0).unwrap(), BERLIN)
}

/// Monday 2026-11-09, 10:00 in Berlin.
fn app() -> App {
    App::new(
        Db::open_in_memory().unwrap(),
        BERLIN,
        Settings::default(),
        at(date(2026, 11, 9), 10, 0),
    )
}

fn add(app: &mut App, title: &str, start: i64, mins: i64) -> Event {
    let mut ev = Event::new(title, start, start + mins * 60);
    ev.tzid = Some("Europe/Berlin".into());
    let ev = app.db.insert(ev).unwrap();
    app.reload();
    ev
}

fn add_all_day(app: &mut App, title: &str, d: NaiveDate) -> Event {
    let mut ev = Event::new(title, time::floating(d), time::floating(d) + DAY);
    ev.all_day = true;
    let ev = app.db.insert(ev).unwrap();
    app.reload();
    ev
}

fn press(app: &mut App, keys: &str) {
    for k in parse_keys(keys).unwrap() {
        app.on_key(k);
    }
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| vcal::tui::ui::render(f, app)).unwrap();
    let buf = term.backend().buffer();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn titles_on(app: &App, d: NaiveDate) -> Vec<String> {
    let from = time::local_midnight(d, BERLIN);
    app.db
        .occurrences(from, from + DAY, BERLIN, false)
        .unwrap()
        .into_iter()
        .map(|o| o.title)
        .collect()
}

#[test]
fn month_navigation() {
    let mut a = app();
    assert_eq!(a.cursor, date(2026, 11, 9));
    press(&mut a, "l");
    assert_eq!(a.cursor, date(2026, 11, 10));
    press(&mut a, "j");
    assert_eq!(a.cursor, date(2026, 11, 17));
    press(&mut a, "kh");
    assert_eq!(a.cursor, date(2026, 11, 9));
    press(&mut a, "w");
    assert_eq!(a.cursor, date(2026, 11, 16));
    press(&mut a, "bb");
    assert_eq!(a.cursor, date(2026, 11, 2));
    press(&mut a, "}");
    assert_eq!(a.cursor, date(2026, 12, 2));
    press(&mut a, "{{");
    assert_eq!(a.cursor, date(2026, 10, 2));
    press(&mut a, "$");
    assert_eq!(
        a.cursor,
        date(2026, 10, 4),
        "Sunday ends a Monday-first week"
    );
    press(&mut a, "0");
    assert_eq!(a.cursor, date(2026, 9, 28));
    press(&mut a, "G");
    assert_eq!(a.cursor, date(2026, 9, 30));
    press(&mut a, "gg");
    assert_eq!(a.cursor, date(2026, 9, 1));
    press(&mut a, "t");
    assert_eq!(a.cursor, date(2026, 11, 9));
    press(&mut a, "}.");
    assert_eq!(a.cursor, date(2026, 11, 9));
}

#[test]
fn chords_wait_and_unknown_ones_reset() {
    let mut a = app();
    press(&mut a, "g");
    assert_eq!(a.pending.len(), 1);
    assert!(screen(&mut a, 100, 30).contains("NORMAL  g"));
    press(&mut a, "x");
    assert!(a.pending.is_empty());
    press(&mut a, "l");
    assert_eq!(
        a.cursor,
        date(2026, 11, 10),
        "keys after a dead chord still work"
    );
}

#[test]
fn view_switching() {
    let mut a = app();
    press(&mut a, "<Tab>");
    assert_eq!(a.view, View::Week);
    press(&mut a, "<Tab>");
    assert_eq!(a.view, View::Day);
    press(&mut a, "<Tab>");
    assert_eq!(a.view, View::Month);
    press(&mut a, "<S-Tab>");
    assert_eq!(a.view, View::Day);
    press(&mut a, "g2");
    assert_eq!(a.view, View::Week);
    press(&mut a, "g1");
    assert_eq!(a.view, View::Month);
}

#[test]
fn week_view_slots_and_today() {
    let mut a = app();
    press(&mut a, "g2");
    assert_eq!(a.slot, 20, "10:00 is slot 20 at 30 minutes");
    press(&mut a, "jjj");
    assert_eq!(a.slot, 23);
    press(&mut a, "l");
    assert_eq!((a.cursor, a.slot), (date(2026, 11, 10), 23));
    press(&mut a, "g3G");
    assert_eq!(a.slot, 47);
    press(&mut a, "gg");
    assert_eq!(a.slot, 0);
    press(&mut a, "k");
    assert_eq!(a.slot, 0, "clamped at midnight");
    press(&mut a, "t");
    assert_eq!((a.cursor, a.slot), (date(2026, 11, 9), 20));
}

#[test]
fn create_event_with_o() {
    let mut a = app();
    press(&mut a, "g2jj"); // week view, 11:00
    press(&mut a, "o");
    assert_eq!(a.mode, Mode::Insert);
    press(&mut a, "Lunch<Tab><Tab><C-u>45m<Tab><Tab>food<CR>");
    assert_eq!(
        a.mode,
        Mode::Normal,
        "{:?}",
        a.form.as_ref().and_then(|f| f.error.clone())
    );
    let occ = a.db.occurrences(0, i64::MAX / 2, BERLIN, false).unwrap();
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].title, "Lunch");
    assert_eq!(occ[0].start, at(date(2026, 11, 9), 11, 0));
    assert_eq!(occ[0].end - occ[0].start, 45 * 60);
    assert_eq!(occ[0].category.as_deref(), Some("food"));
    assert!(a.message.as_ref().unwrap().text.contains("created"));
}

#[test]
fn form_errors_keep_the_dialog() {
    let mut a = app();
    press(&mut a, "o<CR>");
    assert_eq!(a.mode, Mode::Insert);
    assert!(
        a.form
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("title")
    );
    let shown = screen(&mut a, 100, 30);
    assert!(shown.contains("New event"), "{shown}");
    assert!(shown.contains("a title is required"));
    press(&mut a, "<Esc>");
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.form.is_none());
}

#[test]
fn all_day_with_capital_o() {
    let mut a = app();
    press(&mut a, "lO");
    press(&mut a, "Holiday<CR>");
    let occ = a.db.occurrences(0, i64::MAX / 2, BERLIN, false).unwrap();
    assert!(occ[0].all_day);
    assert_eq!(time::floating_date(occ[0].start), date(2026, 11, 10));
}

#[test]
fn delete_needs_confirmation() {
    let mut a = app();
    add(&mut a, "Dentist", at(date(2026, 11, 9), 14, 0), 60);
    press(&mut a, "dd");
    assert_eq!(a.mode, Mode::Prompt);
    assert!(screen(&mut a, 100, 30).contains("Delete \"Dentist\"?"));
    press(&mut a, "n");
    assert_eq!(titles_on(&a, date(2026, 11, 9)), ["Dentist"]);
    press(&mut a, "ddy");
    assert!(titles_on(&a, date(2026, 11, 9)).is_empty());
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn recurring_delete_asks_for_scope() {
    let mut a = app();
    let mut ev = Event::new(
        "Standup",
        at(date(2026, 11, 9), 9, 0),
        at(date(2026, 11, 9), 9, 15),
    );
    ev.rrule = Some("FREQ=DAILY".into());
    a.db.insert(ev).unwrap();
    a.reload();

    press(&mut a, "ldd");
    assert!(screen(&mut a, 100, 30).contains("repeats"));
    press(&mut a, "oy");
    assert!(titles_on(&a, date(2026, 11, 10)).is_empty());
    assert_eq!(titles_on(&a, date(2026, 11, 11)), ["Standup"]);

    press(&mut a, "ldda");
    assert!(screen(&mut a, 100, 30).contains("every occurrence"));
    press(&mut a, "y");
    assert!(titles_on(&a, date(2026, 11, 9)).is_empty());
    assert!(titles_on(&a, date(2026, 11, 12)).is_empty());
}

#[test]
fn change_edits_in_place() {
    let mut a = app();
    let ev = add(&mut a, "Dentist", at(date(2026, 11, 9), 14, 0), 60);
    press(&mut a, "cc");
    assert_eq!(a.mode, Mode::Insert);
    press(&mut a, "<C-u>Orthodontist<CR>");
    let got = a.db.get(&ev.id).unwrap().unwrap();
    assert_eq!(got.title, "Orthodontist");
    assert_eq!(got.start, ev.start, "untouched fields stay");
}

#[test]
fn edit_one_occurrence() {
    let mut a = app();
    let mut ev = Event::new(
        "Gym",
        at(date(2026, 11, 9), 18, 0),
        at(date(2026, 11, 9), 19, 0),
    );
    ev.rrule = Some("FREQ=WEEKLY".into());
    a.db.insert(ev).unwrap();
    a.reload();
    press(&mut a, "cco<C-u>Gym (short)<Tab><Tab><C-u>30m<CR>");
    let occ =
        a.db.occurrences(
            at(date(2026, 11, 9), 0, 0),
            at(date(2026, 11, 17), 0, 0),
            BERLIN,
            false,
        )
        .unwrap();
    assert_eq!(occ[0].title, "Gym (short)");
    assert_eq!(occ[0].end - occ[0].start, 1800);
    assert_eq!(occ[1].title, "Gym");
}

#[test]
fn yank_and_paste() {
    let mut a = app();
    add(&mut a, "Call", at(date(2026, 11, 9), 14, 0), 30);
    press(&mut a, "yywp");
    let occ =
        a.db.occurrences(
            at(date(2026, 11, 16), 0, 0),
            at(date(2026, 11, 17), 0, 0),
            BERLIN,
            false,
        )
        .unwrap();
    assert_eq!(occ.len(), 1);
    assert_eq!(
        occ[0].start,
        at(date(2026, 11, 16), 14, 0),
        "month view keeps the time of day"
    );

    // week view pastes at the cursor slot
    press(&mut a, "g2lp");
    let pasted =
        a.db.occurrences(
            at(date(2026, 11, 17), 0, 0),
            at(date(2026, 11, 18), 0, 0),
            BERLIN,
            false,
        )
        .unwrap();
    assert_eq!(
        pasted[0].start,
        at(date(2026, 11, 17), 10, 0),
        "the cursor slot, 10:00"
    );
}

#[test]
fn visual_month_delete_and_new() {
    let mut a = app();
    add(&mut a, "A", at(date(2026, 11, 9), 9, 0), 60);
    add(&mut a, "B", at(date(2026, 11, 10), 9, 0), 60);
    add(&mut a, "C", at(date(2026, 11, 12), 9, 0), 60);
    press(&mut a, "vl");
    assert_eq!(a.mode, Mode::Visual);
    assert!(screen(&mut a, 100, 30).contains("VISUAL"));
    press(&mut a, "d");
    assert!(screen(&mut a, 100, 30).contains("Delete 2 events?"));
    press(&mut a, "y");
    assert!(titles_on(&a, date(2026, 11, 9)).is_empty());
    assert!(titles_on(&a, date(2026, 11, 10)).is_empty());
    assert_eq!(titles_on(&a, date(2026, 11, 12)), ["C"]);

    // o over a selection makes a multi-day all-day event
    press(&mut a, "vllo");
    let form = a.form.as_ref().unwrap();
    assert!(form.all_day);
    assert_eq!(
        (form.start.text.as_str(), form.end.text.as_str()),
        ("2026-11-10", "2026-11-12")
    );
}

#[test]
fn visual_week_shift() {
    let mut a = app();
    let ev = add(&mut a, "Focus", at(date(2026, 11, 9), 10, 0), 60);
    press(&mut a, "g2vj>>");
    let got = a.db.get(&ev.id).unwrap().unwrap();
    assert_eq!(got.start, at(date(2026, 11, 9), 11, 0), "two slots later");
    assert_eq!(a.slot, 23, "the selection follows");
    press(&mut a, "<lt><Esc>");
    assert_eq!(
        a.db.get(&ev.id).unwrap().unwrap().start,
        at(date(2026, 11, 9), 10, 30)
    );
    assert_eq!(a.mode, Mode::Normal);
    assert!(a.anchor.is_none());
}

#[test]
fn search_filters_and_jumps() {
    let mut a = app();
    add(&mut a, "Dentist", at(date(2026, 11, 9), 14, 0), 60);
    add(&mut a, "Lunch", at(date(2026, 11, 9), 12, 0), 60);
    add(&mut a, "Dentist follow-up", at(date(2026, 12, 3), 9, 0), 30);
    press(&mut a, "/dent");
    assert_eq!(a.mode, Mode::Search);
    assert_eq!(a.day_events(a.cursor).len(), 1, "filters live");
    press(&mut a, "<CR>n");
    assert_eq!(a.cursor, date(2026, 12, 3));
    press(&mut a, "N");
    assert_eq!(a.cursor, date(2026, 11, 9));
    press(&mut a, "<Esc>");
    assert_eq!(a.filter, None);
    assert_eq!(a.day_events(a.cursor).len(), 2);
}

#[test]
fn ex_commands() {
    let mut a = app();
    press(&mut a, ":jump 2027-02-14<CR>");
    assert_eq!(a.cursor, date(2027, 2, 14));
    press(&mut a, ":view day<CR>");
    assert_eq!(a.view, View::Day);
    press(&mut a, ":today<CR>");
    assert_eq!(a.cursor, date(2026, 11, 9));
    press(&mut a, ":2026-12-24 18:00<CR>");
    assert_eq!((a.cursor, a.slot), (date(2026, 12, 24), 36));
    press(&mut a, ":frobnicate<CR>");
    assert!(a.message.as_ref().unwrap().error);
    press(&mut a, ":w<CR>");
    assert!(!a.quit);
    press(&mut a, ":<BS>");
    assert_eq!(a.mode, Mode::Normal, "backspace on an empty line leaves");
    press(&mut a, ":q<CR>");
    assert!(a.quit);
}

#[test]
fn escape_variants() {
    let mut a = app();
    add(&mut a, "X", at(date(2026, 11, 9), 14, 0), 60);
    press(&mut a, "<CR>");
    assert_eq!(a.focus, Focus::Events);
    press(&mut a, "<C-[>");
    assert_eq!(a.focus, Focus::Grid);
    press(&mut a, "v<C-c>");
    assert_eq!(a.mode, Mode::Normal);
}

#[test]
fn event_list_focus() {
    let mut a = app();
    add(&mut a, "First", at(date(2026, 11, 9), 9, 0), 60);
    add(&mut a, "Second", at(date(2026, 11, 9), 11, 0), 60);
    press(&mut a, "<CR>j");
    assert_eq!(a.target().unwrap().title, "Second");
    press(&mut a, "k");
    assert_eq!(a.target().unwrap().title, "First");
    press(&mut a, "<CR>");
    assert_eq!(a.mode, Mode::Insert, "Enter on a listed event edits it");
}

#[test]
fn renders_month() {
    let mut a = app();
    add(&mut a, "Dentist", at(date(2026, 11, 10), 14, 0), 60);
    add_all_day(&mut a, "Release", date(2026, 11, 11));
    let mut h = Event::new(
        "Armistice",
        time::floating(date(2026, 11, 11)),
        time::floating(date(2026, 11, 12)),
    );
    h.all_day = true;
    h.category = Some("holiday".into());
    a.db.insert(h).unwrap();
    a.reload();
    let s = screen(&mut a, 120, 40);
    assert!(s.contains("November 2026"), "{s}");
    assert!(s.contains("Mon") && s.contains("Sun"));
    assert!(s.contains("14:00 Dent"), "{s}");
    assert!(s.contains("▌Release"));
    assert!(s.contains("11 ★"), "holiday marker\n{s}");
    assert!(s.contains("Mon 9 November"), "agenda panel");
    assert!(s.contains(" NORMAL "));
}

#[test]
fn renders_week_blocks_on_the_right_rows() {
    let mut a = app();
    add(&mut a, "Planning", at(date(2026, 11, 11), 13, 0), 90);
    press(&mut a, "g2");
    let s = screen(&mut a, 120, 40);
    assert!(s.contains("Week 46"), "{s}");
    let lines: Vec<&str> = s.lines().collect();
    let row_13 = lines
        .iter()
        .position(|l| l.starts_with("13:00"))
        .expect("13:00 row");
    assert!(lines[row_13].contains("13:00 Planning"), "{s}");
    assert!(!lines[row_13 - 1].contains("Planning"));
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("10:00") && l.contains('▶')),
        "now marker\n{s}"
    );
}

#[test]
fn renders_day_agenda() {
    let mut a = app();
    let mut ev = Event::new(
        "Workshop",
        at(date(2026, 11, 9), 13, 0),
        at(date(2026, 11, 9), 14, 30),
    );
    ev.description = "Bring laptop".into();
    ev.location = Some("Room 4".into());
    a.db.insert(ev).unwrap();
    a.reload();
    press(&mut a, "g3");
    let s = screen(&mut a, 120, 40);
    assert!(s.contains("Monday 9 November 2026"), "{s}");
    assert!(s.contains("13:00–14:30"));
    assert!(s.contains("███ 1h30"), "duration bar\n{s}");
    assert!(s.contains("@ Room 4"));
    assert!(s.contains("Bring laptop"));
}

#[test]
fn renders_help_and_survives_tiny_terminals() {
    let mut a = app();
    press(&mut a, "?");
    let s = screen(&mut a, 120, 50);
    assert!(s.contains("Keys"), "{s}");
    assert!(s.contains("gg"));
    press(&mut a, "x");
    assert_eq!(a.mode, Mode::Normal);
    for view in ["1", "2", "3"] {
        press(&mut a, view);
        for (w, h) in [(10, 5), (40, 8), (1, 1)] {
            screen(&mut a, w, h);
        }
    }
}

#[test]
fn reloads_after_external_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.db");
    let mut a = App::new(
        Db::open(&path).unwrap(),
        BERLIN,
        Settings::default(),
        at(date(2026, 11, 9), 10, 0),
    );
    assert!(a.day_events(a.cursor).is_empty());
    // another process (the CLI, Quickshell) writes
    let mut other = Db::open(&path).unwrap();
    other
        .insert(Event::new(
            "From elsewhere",
            at(date(2026, 11, 9), 15, 0),
            at(date(2026, 11, 9), 16, 0),
        ))
        .unwrap();
    a.tick(a.now + 1);
    assert_eq!(a.day_events(a.cursor)[0].title, "From elsewhere");
}

mod config_reload {
    use super::*;
    use ratatui::style::Color;
    use std::path::Path;

    fn watched(dir: &Path, text: &str) -> (App, std::path::PathBuf) {
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        let cfg = vcal::config::load(&path).unwrap();
        let mut a = app();
        a.apply_config(cfg);
        a.watch_config(path.clone(), None);
        (a, path)
    }

    #[test]
    fn applies_changes_on_tick() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, path) = watched(dir.path(), "");
        assert!(a.settings.clock_24h);
        a.tick(a.now);
        assert!(a.message.is_none(), "no change, no reload");

        std::fs::write(
            &path,
            "[general]\nclock = \"12h\"\n[theme]\nbase0D = \"#123456\"\n[keybindings.normal]\nquit = [\"q\"]\n",
        )
        .unwrap();
        a.tick(a.now);
        assert!(!a.settings.clock_24h);
        assert_eq!(a.theme.accent(), Color::Rgb(0x12, 0x34, 0x56));
        assert_eq!(a.message.as_ref().unwrap().text, "config reloaded");

        // the new colour is what gets drawn: the active tab uses the accent
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| vcal::tui::ui::render(f, &mut a)).unwrap();
        let tab = term
            .backend()
            .buffer()
            .content()
            .iter()
            .find(|c| c.symbol() == "M" && c.bg == Color::Rgb(0x12, 0x34, 0x56));
        assert!(tab.is_some(), "Month tab drawn in the configured accent");

        // and the new keys work
        press(&mut a, "q");
        assert!(a.quit);
    }

    #[test]
    fn broken_config_keeps_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, path) = watched(dir.path(), "[general]\nclock = \"12h\"\n");
        std::fs::write(&path, "[general]\nclock = \"13h\"\n").unwrap();
        a.tick(a.now);
        assert!(!a.settings.clock_24h, "still the last good config");
        let msg = a.message.clone().unwrap();
        assert!(
            msg.error
                && msg.text.contains("[general] clock")
                && msg.text.contains("keeping the previous"),
            "{msg:?}"
        );

        // fixing the file recovers
        std::fs::write(&path, "[general]\nclock = \"24h\"\n").unwrap();
        a.tick(a.now);
        assert!(a.settings.clock_24h);
    }

    #[test]
    fn follows_symlink_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let (gen1, gen2) = (dir.path().join("gen1.toml"), dir.path().join("gen2.toml"));
        std::fs::write(&gen1, "[views]\nstart = \"week\"\n").unwrap();
        std::fs::write(&gen2, "[views]\nstart = \"day\" \n").unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&gen1, &link).unwrap();
        let mut a = app();
        a.apply_config(vcal::config::load(&link).unwrap());
        a.watch_config(link.clone(), None);
        assert_eq!(a.settings.start_view, View::Week);
        // what a Home-Manager switch does
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&gen2, &link).unwrap();
        a.tick(a.now);
        assert_eq!(a.settings.start_view, View::Day);
    }

    #[test]
    fn slot_granularity_change_keeps_the_time() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, path) = watched(dir.path(), "");
        press(&mut a, "g2");
        assert_eq!(a.slot, 20, "10:00 at 30 minutes");
        std::fs::write(&path, "[views]\nslot_minutes = 15\n").unwrap();
        a.tick(a.now);
        assert_eq!(a.slot, 40, "still 10:00 at 15 minutes");
        std::fs::write(&path, "[views]\nslot_minutes = 60\nfirst_hour = 12\n").unwrap();
        a.tick(a.now);
        assert_eq!(a.slot, 12, "clamped into the new visible hours");
    }

    #[test]
    fn database_path_change_needs_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, path) = watched(dir.path(), "");
        std::fs::write(
            &path,
            "[general]\ndatabase_path = \"/elsewhere/events.db\"\n",
        )
        .unwrap();
        a.tick(a.now);
        assert!(
            a.message
                .as_ref()
                .unwrap()
                .text
                .contains("take effect when vcal restarts")
        );
    }
}

#[test]
fn counts_repeat_motions() {
    let mut a = app();
    press(&mut a, "3l");
    assert_eq!(a.cursor, date(2026, 11, 12));
    press(&mut a, "2w");
    assert_eq!(a.cursor, date(2026, 11, 26));
    press(&mut a, "10h");
    assert_eq!(a.cursor, date(2026, 11, 16), "0 continues a count");
    press(&mut a, "0");
    assert_eq!(a.cursor, date(2026, 11, 16), "0 alone is still row_start");
    press(&mut a, "2j");
    assert_eq!(a.cursor, date(2026, 11, 30));
    press(&mut a, "2k");
    assert_eq!(a.cursor, date(2026, 11, 16));

    a.goto_date(date(2027, 1, 31));
    press(&mut a, "3}");
    assert_eq!(a.cursor, date(2027, 4, 30), "one jump, not three clamps");
    press(&mut a, "2{");
    assert_eq!(a.cursor, date(2027, 2, 28));

    // a count shows while it's typed and goes with Esc or a dead key
    press(&mut a, "12");
    assert_eq!(a.count, Some(12));
    assert!(screen(&mut a, 100, 30).contains("NORMAL  12"));
    press(&mut a, "<Esc>l");
    assert_eq!(a.cursor, date(2027, 3, 1));
    press(&mut a, "5xl");
    assert_eq!(a.cursor, date(2027, 3, 2));
    // and is ignored by actions that don't repeat
    press(&mut a, "3g2");
    assert_eq!(a.view, View::Week);
    assert_eq!(a.count, None);
}

#[test]
fn counts_in_week_view_and_visual() {
    let mut a = app();
    let ev = add(&mut a, "Focus", at(date(2026, 11, 9), 10, 0), 60);
    press(&mut a, "g2");
    let slot = a.slot;
    press(&mut a, "4j");
    assert_eq!(a.slot, slot + 4);
    press(&mut a, "4kv3>");
    assert_eq!(
        a.db.get(&ev.id).unwrap().unwrap().start,
        at(date(2026, 11, 9), 11, 30),
        "three slots in one write"
    );
    assert_eq!(a.slot, slot + 3);
}

#[test]
fn quick_add_command() {
    let mut a = app();
    press(&mut a, "lll"); // Thursday the 12th
    press(&mut a, ":add Dentist 14:00 45m @health<CR>");
    assert_eq!(a.mode, Mode::Normal);
    let msg = a.message.clone().unwrap();
    assert!(!msg.error, "{}", msg.text);
    assert_eq!(
        msg.text,
        "created \"Dentist\" Thu 12 Nov 14:00 (45m) @health"
    );
    let o = &a.day_events(date(2026, 11, 12))[0];
    assert_eq!(
        (o.start, o.end),
        (
            at(date(2026, 11, 12), 14, 0),
            at(date(2026, 11, 12), 14, 45)
        )
    );

    // a named day moves the cursor there
    press(&mut a, ":a Trip fri 2d<CR>");
    assert_eq!(a.cursor, date(2026, 11, 13));
    assert!(a.day_events(date(2026, 11, 14))[0].all_day);

    press(&mut a, ":add tomorrow<CR>");
    assert!(a.message.as_ref().unwrap().error);
}
