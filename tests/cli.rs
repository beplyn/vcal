//! The headless CLI, driven as a subprocess the way scripts and Quickshell use it.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn vcal(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vcal"))
        .args(args)
        .env("VCAL_DB", db)
        // never touch the real ~/.config
        .env("XDG_CONFIG_HOME", db.parent().unwrap())
        .env_remove("VCAL_CONFIG")
        .env("TZ", "Europe/Berlin")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(db: &Path, args: &[&str]) -> String {
    let out = vcal(db, args);
    assert!(
        out.status.success(),
        "vcal {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn json(db: &Path, from: &str, to: &str) -> Vec<Value> {
    let out = ok(
        db,
        &["query", "--from", from, "--to", to, "--format", "json"],
    );
    serde_json::from_str::<Value>(&out)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn add_query_rm() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");

    let id = ok(
        &db,
        &[
            "add",
            "-t",
            "Dentist",
            "-s",
            "2026-11-10 14:00",
            "-D",
            "45m",
            "-c",
            "health",
        ],
    );
    let id = id.trim();
    ok(
        &db,
        &[
            "add",
            "--title",
            "Trip",
            "--start",
            "2026-11-12",
            "--end",
            "2026-11-14",
            "--all-day",
        ],
    );
    let series = ok(
        &db,
        &[
            "add",
            "-t",
            "Gym",
            "-s",
            "2026-11-09 18:00",
            "--rrule",
            "FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4",
        ],
    );

    let rows = json(&db, "2026-11-09", "2026-11-15");
    let titles: Vec<_> = rows.iter().map(|r| r["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Gym", "Dentist", "Gym", "Trip"]);

    let dentist = &rows[1];
    assert_eq!(dentist["id"], id);
    assert_eq!(dentist["start"], "2026-11-10T14:00:00+01:00");
    assert_eq!(dentist["end"], "2026-11-10T14:45:00+01:00");
    assert_eq!(dentist["category"], "health");
    assert_eq!(dentist["occurrence"], Value::Null);

    let trip = &rows[3];
    assert_eq!(trip["all_day"], true);
    assert_eq!(trip["start"], "2026-11-12");
    assert_eq!(trip["end"], "2026-11-15");
    assert_eq!(
        trip["end_ts"].as_i64().unwrap() - trip["start_ts"].as_i64().unwrap(),
        3 * 86_400
    );

    // Cancel Wednesday's gym session only.
    let wednesday = rows[2]["occurrence"].as_i64().unwrap().to_string();
    ok(&db, &["rm", series.trim(), "--occurrence", &wednesday]);
    assert_eq!(json(&db, "2026-11-09", "2026-11-15").len(), 3);

    ok(&db, &["rm", id]);
    assert_eq!(json(&db, "2026-11-09", "2026-11-15").len(), 2);
    assert!(!vcal(&db, &["rm", id]).status.success());
}

#[test]
fn text_and_tsv_formats() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    ok(&db, &["add", "-t", "Call\tMum", "-s", "2026-11-10 09:00"]);
    ok(
        &db,
        &[
            "add",
            "-t",
            "Payday",
            "-s",
            "2026-11-10",
            "-a",
            "-c",
            "money",
        ],
    );

    let text = ok(&db, &["query", "--from", "2026-11-10"]);
    assert_eq!(
        text,
        "Tue 2026-11-10  all day      Payday  [money]\n                \
         09:00–10:00  Call Mum\n"
    );
    let tsv = ok(&db, &["query", "--from", "2026-11-10", "--format", "tsv"]);
    let lines: Vec<_> = tsv.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with("id\toccurrence\tstart"));
    assert!(lines[2].ends_with("\tCall Mum"));
}

#[test]
fn input_errors_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    for args in [
        &["add", "-t", "x", "-s", "whenever"][..],
        &["add", "-t", "x", "-s", "2026-11-10"],
        &["add", "-t", "x", "-s", "2026-11-10", "-a", "-D", "3h"],
        &[
            "add",
            "-t",
            "x",
            "-s",
            "2026-11-10 09:00",
            "-r",
            "FREQ=NEVER",
        ],
        &["query", "--from", "2026-11-10", "--to", "2026-11-01"],
    ] {
        let out = vcal(&db, args);
        assert!(!out.status.success(), "{args:?} should fail");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("vcal: "));
    }
}

#[test]
fn import_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let ics = dir.path().join("in.ics");
    std::fs::write(
        &ics,
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Imported\r\n\
         DTSTART:20261110T080000Z\r\nDURATION:PT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
    )
    .unwrap();
    let out = vcal(&db, &["import", ics.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("imported 1 events"));
    let rows = json(&db, "2026-11-10", "2026-11-10");
    assert_eq!(rows[0]["id"], "a");
    assert_eq!(rows[0]["start"], "2026-11-10T09:00:00+01:00");
}

#[test]
fn gen_assets_writes_man_and_completions() {
    let dir = tempfile::tempdir().unwrap();
    ok(
        &dir.path().join("events.db"),
        &["gen-assets", dir.path().join("out").to_str().unwrap()],
    );
    for f in ["vcal.1", "vcal.bash", "vcal.fish", "_vcal"] {
        assert!(
            dir.path().join("out").join(f).metadata().unwrap().len() > 100,
            "{f}"
        );
    }
}

#[test]
fn creates_db_under_xdg_data_home() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vcal"))
        .args(["query", "--format", "json"])
        .env_remove("VCAL_DB")
        .env_remove("VCAL_CONFIG")
        .env("XDG_DATA_HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("TZ", "UTC")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
    assert!(dir.path().join("calendar/events.db").exists());
}

fn err(db: &Path, args: &[&str]) -> String {
    let out = vcal(db, args);
    assert!(!out.status.success(), "vcal {args:?} should fail");
    String::from_utf8(out.stderr).unwrap()
}

fn one(db: &Path, day: &str) -> Value {
    let rows = json(db, day, day);
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0].clone()
}

#[test]
fn edit_single_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let id = ok(
        &db,
        &[
            "add",
            "-t",
            "Dentist",
            "-s",
            "2026-11-10 14:00",
            "-D",
            "45m",
            "-c",
            "health",
            "-l",
            "Dr. Lee",
        ],
    );
    let id = id.trim();

    assert_eq!(ok(&db, &["edit", id, "--title", "Orthodontist"]).trim(), id);
    let row = one(&db, "2026-11-10");
    assert_eq!(
        (row["title"].as_str(), row["category"].as_str()),
        (Some("Orthodontist"), Some("health"))
    );

    // a lone --start moves the event and keeps its 45 minutes
    ok(&db, &["edit", id, "--start", "2026-11-11 09:00"]);
    let row = one(&db, "2026-11-11");
    assert_eq!(row["start"], "2026-11-11T09:00:00+01:00");
    assert_eq!(row["end"], "2026-11-11T09:45:00+01:00");

    ok(
        &db,
        &[
            "edit",
            id,
            "--duration",
            "2h",
            "--category",
            "",
            "--location",
            "",
        ],
    );
    let row = one(&db, "2026-11-11");
    assert_eq!(row["end"], "2026-11-11T11:00:00+01:00");
    assert_eq!(
        (row["category"].clone(), row["location"].clone()),
        (Value::Null, Value::Null)
    );

    ok(&db, &["edit", id, "--all-day", "--end", "2026-11-12"]);
    let row = one(&db, "2026-11-12");
    assert_eq!(
        (
            row["all_day"].as_bool(),
            row["start"].as_str(),
            row["end"].as_str()
        ),
        (Some(true), Some("2026-11-11"), Some("2026-11-13"))
    );

    assert!(err(&db, &["edit", id, "--timed"]).contains("needs --start with a time"));
    ok(&db, &["edit", id, "--timed", "--start", "2026-11-11 15:00"]);
    let row = one(&db, "2026-11-11");
    assert_eq!(
        (row["all_day"].as_bool(), row["end"].as_str()),
        (Some(false), Some("2026-11-11T16:00:00+01:00"))
    );

    ok(&db, &["edit", id, "--status", "tentative"]);
    assert_eq!(one(&db, "2026-11-11")["status"], "tentative");

    assert!(err(&db, &["edit", id]).contains("nothing to change"));
    assert!(err(&db, &["edit", "nope", "-t", "x"]).contains("no event with id nope"));
    assert!(
        err(&db, &["edit", id, "--end", "2026-11-11 10:00"]).contains("end is before the start")
    );
}

#[test]
fn edit_series_and_occurrences() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let id = ok(
        &db,
        &[
            "add",
            "-t",
            "Standup",
            "-s",
            "2026-11-09 09:30",
            "-D",
            "15m",
            "-r",
            "FREQ=DAILY;COUNT=5",
            "-c",
            "work",
        ],
    );
    let id = id.trim();

    // one occurrence: retitle it, then move it; the retitle survives the move
    let tuesday = one(&db, "2026-11-10")["occurrence"]
        .as_i64()
        .unwrap()
        .to_string();
    ok(
        &db,
        &[
            "edit",
            id,
            "--occurrence",
            &tuesday,
            "--title",
            "Standup (remote)",
        ],
    );
    ok(
        &db,
        &[
            "edit",
            id,
            "--occurrence",
            &tuesday,
            "--start",
            "2026-11-10 11:00",
        ],
    );
    let row = one(&db, "2026-11-10");
    assert_eq!(row["title"], "Standup (remote)");
    assert_eq!(row["start"], "2026-11-10T11:00:00+01:00");
    assert_eq!(
        row["end"], "2026-11-10T11:15:00+01:00",
        "keeps the series' 15 minutes"
    );
    assert_eq!(
        row["occurrence"].as_i64().unwrap().to_string(),
        tuesday,
        "same handle after moving"
    );
    assert_eq!(row["modified"], true);
    assert_eq!(
        one(&db, "2026-11-11")["title"],
        "Standup",
        "other days untouched"
    );

    // "" on an occurrence falls back to the series value
    ok(
        &db,
        &["edit", id, "--occurrence", &tuesday, "--category", "home"],
    );
    assert_eq!(one(&db, "2026-11-10")["category"], "home");
    ok(
        &db,
        &["edit", id, "--occurrence", &tuesday, "--category", ""],
    );
    assert_eq!(one(&db, "2026-11-10")["category"], "work");

    // whole series: retitle all
    ok(&db, &["edit", id, "--title", "Daily"]);
    assert_eq!(one(&db, "2026-11-12")["title"], "Daily");
    assert_eq!(
        one(&db, "2026-11-10")["title"],
        "Standup (remote)",
        "the exception still wins"
    );

    // bad handles and series-only flags
    assert!(
        err(&db, &["edit", id, "--occurrence", "12345", "-t", "x"])
            .contains("no occurrence at 12345")
    );
    let wednesday = one(&db, "2026-11-11")["occurrence"]
        .as_i64()
        .unwrap()
        .to_string();
    ok(&db, &["rm", id, "--occurrence", &wednesday]);
    assert!(err(&db, &["edit", id, "--occurrence", &wednesday, "-t", "x"]).contains("cancelled"));
    assert!(
        err(&db, &["edit", id, "--occurrence", &tuesday, "--rrule", ""])
            .contains("cannot be used with")
    );

    // dropping the rule makes it a single event on its first day
    ok(&db, &["edit", id, "--rrule", ""]);
    assert_eq!(json(&db, "2026-11-09", "2026-11-14").len(), 1);
    let single = ok(&db, &["add", "-t", "Once", "-s", "2026-11-20 10:00"]);
    assert!(
        err(
            &db,
            &["edit", single.trim(), "--occurrence", "1", "-t", "x"]
        )
        .contains("doesn't repeat")
    );
}

#[test]
fn writes_and_reads_the_config() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let config = dir.path().join("calendar/config.toml");

    assert_eq!(
        ok(&db, &["config", "path"]).trim(),
        config.to_str().unwrap()
    );
    assert!(ok(&db, &["config", "check"]).contains("the defaults apply"));
    assert!(!config.exists(), "inspecting doesn't create it");

    // the first real command writes the documented default and says so
    let out = vcal(&db, &["query"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("wrote the default config"));
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        ok(&db, &["config", "default"])
    );
    assert!(ok(&db, &["config", "check"]).starts_with("ok: "));
    assert!(
        String::from_utf8_lossy(&vcal(&db, &["query"]).stderr).is_empty(),
        "only once"
    );

    // settings reach the CLI
    std::fs::write(
        &config,
        "[general]\nclock = \"12h\"\ndefault_duration = \"20m\"\n",
    )
    .unwrap();
    ok(&db, &["add", "-t", "Tea", "-s", "2026-11-10 15:00"]);
    assert_eq!(
        ok(&db, &["query", "--from", "2026-11-10"]),
        "Tue 2026-11-10  3:00pm–3:20pm  Tea\n"
    );

    // a broken file is an error that names the key
    std::fs::write(&config, "[views]\nslot_minutes = 7\n").unwrap();
    assert!(err(&db, &["config", "check"]).contains("slot_minutes"));
    assert!(err(&db, &["query"]).contains("slot_minutes"));
}

#[test]
fn database_path_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let from_config = dir.path().join("configured.db");
    std::fs::create_dir_all(dir.path().join("calendar")).unwrap();
    std::fs::write(
        dir.path().join("calendar/config.toml"),
        format!(
            "[general]\ndatabase_path = {:?}\n",
            from_config.to_str().unwrap()
        ),
    )
    .unwrap();
    let run = |db: Option<&Path>| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vcal"));
        cmd.args(["add", "-t", "x", "-s", "2026-11-10 10:00"])
            .env_remove("VCAL_DB")
            .env_remove("VCAL_CONFIG")
            .env("XDG_CONFIG_HOME", dir.path())
            .env("XDG_DATA_HOME", dir.path().join("data"));
        if let Some(db) = db {
            cmd.env("VCAL_DB", db);
        }
        assert!(cmd.output().unwrap().status.success());
    };
    run(None);
    assert!(from_config.exists(), "config's database_path");
    let env_db = dir.path().join("env.db");
    run(Some(&env_db));
    assert!(env_db.exists(), "$VCAL_DB wins");
    assert!(!dir.path().join("data/calendar/events.db").exists());
}

#[test]
fn next_for_status_bars() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    assert_eq!(ok(&db, &["next"]), "", "nothing ahead prints nothing");

    ok(&db, &["add", "-t", "Trip", "-s", "today", "--all-day"]);
    ok(&db, &["add", "-t", "Ongoing", "--start=-10m", "-D", "1h"]);
    ok(
        &db,
        &[
            "add", "-t", "Later", "-s", "+3h", "-D", "30m", "-l", "Room 4",
        ],
    );
    ok(&db, &["add", "-t", "Soon", "-s", "+30m", "-D", "15m"]);

    assert!(ok(&db, &["next"]).ends_with(" Soon\n"));
    assert_eq!(
        ok(&db, &["next", "--format", "{title} {in} {{x}}"]),
        "Soon in 30m {x}\n"
    );
    assert!(ok(&db, &["next", "--ongoing", "--format", "{title}|{in}"]).starts_with("Ongoing|now"));
    assert!(
        ok(
            &db,
            &["next", "--all-day", "--ongoing", "--format", "{title}"]
        )
        .starts_with("Trip")
    );
    assert_eq!(ok(&db, &["next", "--within", "20m"]), "");

    let row: Value = serde_json::from_str(&ok(&db, &["next", "--json"])).unwrap();
    assert_eq!(row["title"], "Soon");

    let start = ok(&db, &["next", "--format", "{start:%Y-%m-%dT%H:%M}"]);
    assert_eq!(start.trim().len(), 16);

    for bad in ["{nope}", "{start:%Q}", "{title", "x}"] {
        let out = vcal(&db, &["next", "--format", bad]);
        assert!(!out.status.success(), "{bad} should fail");
    }
}

#[test]
fn reminders_notify_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = dir.path().join("sent.log");
    let fake = bin.join("notify-send");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s|' \"$@\" >> {}\necho >> {}\n",
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let notify = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_vcal"))
            .arg("notify")
            .args(args)
            .env("VCAL_DB", &db)
            .env("XDG_CONFIG_HOME", dir.path())
            .env_remove("VCAL_CONFIG")
            .env("TZ", "Europe/Berlin")
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };

    let due = ok(
        &db,
        &[
            "add", "-t", "Dentist", "-s", "+10m", "-R", "15m", "-l", "Dr. Lee",
        ],
    );
    ok(&db, &["add", "-t", "Not yet", "-s", "+1h", "-R", "15m"]);
    ok(&db, &["add", "-t", "No reminder", "-s", "+5m"]);
    ok(&db, &["add", "-t", "Long gone", "--start=-1h", "-R", "5m"]);

    let rows = json(&db, "today", "+2d");
    let dentist = rows.iter().find(|r| r["title"] == "Dentist").unwrap();
    assert_eq!(dentist["reminder"], 15);

    let dry = notify(&["--dry-run"]);
    assert!(dry.starts_with("Dentist\t"), "{dry}");
    assert!(dry.contains("· Dr. Lee"));
    assert_eq!(dry.lines().count(), 1);
    assert!(!log.exists(), "a dry run sends nothing");

    notify(&[]);
    notify(&[]);
    let sent = std::fs::read_to_string(&log).unwrap();
    assert_eq!(sent.lines().count(), 1, "sent once: {sent}");
    assert!(sent.starts_with("--app-name=vcal|--icon=x-office-calendar|--|Dentist|"));
    assert_eq!(notify(&["--dry-run"]), "", "already sent");

    // removing the reminder; "" clears it
    ok(&db, &["edit", due.trim(), "--reminder", ""]);
    let rows = json(&db, "today", "+2d");
    let dentist = rows.iter().find(|r| r["title"] == "Dentist").unwrap();
    assert!(dentist["reminder"].is_null());
    assert!(
        !vcal(&db, &["add", "-t", "x", "-s", "+1h", "-R", "30s"])
            .status
            .success()
    );
}
