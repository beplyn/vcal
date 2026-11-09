//! The headless CLI, driven as a subprocess the way scripts and Quickshell use it.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn vcal(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vcal"))
        .args(args)
        .env("VCAL_DB", db)
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
        .env("XDG_DATA_HOME", dir.path())
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
