//! `vcal sync` against an in-process HTTP server that plays an iCalendar
//! feed and a small CalDAV collection (calendar-query, calendar-multiget,
//! PUT and DELETE with ETag preconditions, Basic auth).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::Value;

#[derive(Default)]
struct State {
    /// href → (etag number, body)
    items: BTreeMap<String, (u64, String)>,
    feed: Option<(u64, String)>,
    next_etag: u64,
    /// "METHOD /path" of every request, in order
    log: Vec<String>,
}

impl State {
    fn put(&mut self, href: &str, body: &str) {
        self.next_etag += 1;
        self.items
            .insert(href.to_string(), (self.next_etag, body.to_string()));
    }
}

const AUTH: &str = "Basic YWxpY2U6c2VjcmV0"; // alice:secret

struct Server {
    url: String,
    state: Arc<Mutex<State>>,
}

fn respond(stream: &mut impl Write, status: &str, headers: &[(&str, String)], body: &str) {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    let _ = stream.write_all(out.as_bytes());
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn multistatus(entries: &[(String, u64, Option<String>)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\"?>\n<d:multistatus xmlns:d=\"DAV:\" xmlns:cal=\"urn:ietf:params:xml:ns:caldav\">\n",
    );
    // the collection itself, as some servers list it
    out.push_str("<d:response><d:href>/cal/</d:href><d:propstat><d:prop/><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>\n");
    for (href, etag, data) in entries {
        out.push_str(&format!(
            "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>\"{etag}\"</d:getetag>"
        ));
        if let Some(d) = data {
            out.push_str(&format!(
                "<cal:calendar-data>{}</cal:calendar-data>",
                xml_escape(d)
            ));
        }
        out.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>\n");
    }
    out.push_str("</d:multistatus>\n");
    out
}

fn handle(stream: std::net::TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).unwrap();
    let body = String::from_utf8(body).unwrap();
    let mut out = stream;
    let mut st = state.lock().unwrap();
    st.log.push(format!("{method} {path}"));
    let etag_of = |n: u64| format!("\"{n}\"");

    if path.starts_with("/feed.ics") {
        match &st.feed {
            Some((etag, _)) if headers.get("if-none-match") == Some(&etag_of(*etag)) => {
                respond(&mut out, "304 Not Modified", &[], "")
            }
            Some((etag, text)) => respond(&mut out, "200 OK", &[("ETag", etag_of(*etag))], text),
            None => respond(&mut out, "404 Not Found", &[], ""),
        }
        return;
    }
    if headers.get("authorization").map(String::as_str) != Some(AUTH) {
        return respond(
            &mut out,
            "401 Unauthorized",
            &[("WWW-Authenticate", "Basic".into())],
            "",
        );
    }
    let current = st.items.get(&path).map(|(e, _)| etag_of(*e));
    let precondition = match (headers.get("if-match"), headers.get("if-none-match")) {
        (Some(want), _) => current.as_ref() == Some(want),
        (None, Some(star)) if star == "*" => current.is_none(),
        _ => true,
    };
    match method.as_str() {
        "REPORT" if body.contains("calendar-multiget") => {
            let hrefs: Vec<String> = body
                .split("<d:href>")
                .skip(1)
                .filter_map(|s| s.split("</d:href>").next())
                .map(str::to_string)
                .collect();
            let entries: Vec<_> = hrefs
                .into_iter()
                .filter_map(|h| {
                    st.items
                        .get(&h)
                        .map(|(e, d)| (h.clone(), *e, Some(d.clone())))
                })
                .collect();
            respond(&mut out, "207 Multi-Status", &[], &multistatus(&entries));
        }
        "REPORT" => {
            let entries: Vec<_> = st
                .items
                .iter()
                .map(|(h, (e, _))| (h.clone(), *e, None))
                .collect();
            respond(&mut out, "207 Multi-Status", &[], &multistatus(&entries));
        }
        "PUT" if !precondition => respond(&mut out, "412 Precondition Failed", &[], ""),
        "PUT" => {
            st.put(&path, &body);
            let etag = etag_of(st.next_etag);
            respond(&mut out, "201 Created", &[("ETag", etag)], "");
        }
        "DELETE" if current.is_none() => respond(&mut out, "404 Not Found", &[], ""),
        "DELETE" if !precondition => respond(&mut out, "412 Precondition Failed", &[], ""),
        "DELETE" => {
            st.items.remove(&path);
            respond(&mut out, "204 No Content", &[], "");
        }
        _ => respond(&mut out, "405 Method Not Allowed", &[], ""),
    }
}

fn server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(State::default()));
    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            handle(stream, &shared);
        }
    });
    Server { url, state }
}

struct Home {
    _dir: tempfile::TempDir,
    db: PathBuf,
    config: PathBuf,
}

fn home() -> Home {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("events.db");
    let config = dir.path().join("calendar/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    Home {
        db,
        config,
        _dir: dir,
    }
}

fn vcal(h: &Home, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vcal"))
        .args(args)
        .env("VCAL_DB", &h.db)
        .env("VCAL_CONFIG", &h.config)
        .env("TZ", "Europe/Berlin")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(h: &Home, args: &[&str]) -> String {
    let out = vcal(h, args);
    assert!(
        out.status.success(),
        "vcal {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn err(h: &Home, args: &[&str]) -> String {
    let out = vcal(h, args);
    assert!(!out.status.success(), "vcal {args:?} should fail");
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

fn rows(h: &Home) -> Vec<Value> {
    let out = ok(
        h,
        &[
            "query",
            "--from",
            "2026-11-01",
            "--to",
            "2026-12-01",
            "--format",
            "json",
        ],
    );
    serde_json::from_str::<Vec<Value>>(&out).unwrap()
}

fn titles(h: &Home) -> Vec<String> {
    rows(h)
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

fn vevent(uid: &str, title: &str, day: u32) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{title}\r\n\
         DTSTART:202611{day:02}T090000Z\r\nDTEND:202611{day:02}T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

fn write_config(h: &Home, text: &str) {
    std::fs::write(&h.config, text).unwrap();
}

#[test]
fn feeds_are_read_only_copies() {
    let s = server();
    let h = home();
    s.state.lock().unwrap().feed = Some((1, vevent("a", "Holiday", 20)));
    write_config(
        &h,
        &format!(
            "[calendars.holidays]\ntype = \"ics\"\nurl = \"{}/feed.ics?token=SECRET\"\ncategory = \"holiday\"\n",
            s.url
        ),
    );
    assert!(ok(&h, &["sync"]).contains("holidays  1 events (1 new, 0 removed)"));
    let r = &rows(&h)[0];
    assert_eq!(r["id"], "holidays:a");
    assert_eq!(r["calendar"], "holidays");
    assert_eq!(r["category"], "holiday");

    assert!(
        ok(&h, &["sync"]).contains("unchanged (1 events)"),
        "304 the second time"
    );

    let e = err(&h, &["edit", "holidays:a", "--title", "Mine"]);
    assert!(e.contains("read-only calendar \"holidays\""), "{e}");
    assert!(err(&h, &["rm", "holidays:a"]).contains("read-only"));

    // the feed changes: one event replaced by another
    s.state.lock().unwrap().feed = Some((2, vevent("b", "Other holiday", 21)));
    assert!(ok(&h, &["sync"]).contains("1 events (1 new, 1 removed)"));
    assert_eq!(titles(&h), ["Other holiday"]);

    let list = ok(&h, &["calendars"]);
    assert!(
        list.contains("holidays  (ics, read-only)  1 events, synced 0m ago"),
        "{list}"
    );
    assert!(
        list.contains("/feed.ics?…") && !list.contains("SECRET"),
        "{list}"
    );

    // a feed that fails says so, without the token
    s.state.lock().unwrap().feed = None;
    let e = err(&h, &["sync"]);
    assert!(e.contains("HTTP 404") && !e.contains("SECRET"), "{e}");
    assert!(ok(&h, &["calendars"]).contains("FAILED: HTTP 404"));

    // dropped from the config: its events go
    write_config(&h, "");
    // sync needs some calendar; give it an unrelated, empty file feed
    let empty = h.db.with_file_name("empty.ics");
    std::fs::write(&empty, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    write_config(
        &h,
        &format!(
            "[calendars.local]\ntype = \"ics\"\nurl = \"{}\"\n",
            empty.display()
        ),
    );
    let out = ok(&h, &["sync"]);
    assert!(
        out.contains("holidays  no longer configured; removed its 1 events"),
        "{out}"
    );
    assert!(titles(&h).is_empty());
}

fn caldav_config(h: &Home, url: &str, extra: &str) {
    write_config(
        h,
        &format!(
            "[calendars.nc]\ntype = \"caldav\"\nurl = \"{url}/cal\"\nusername = \"alice\"\n\
             password_command = \"echo secret\"\n{extra}"
        ),
    );
}

#[test]
fn caldav_two_way() {
    let s = server();
    let h = home();
    caldav_config(&h, &s.url, "default = true\n");
    s.state
        .lock()
        .unwrap()
        .put("/cal/remote.ics", &vevent("remote", "From phone", 10));

    // a new event goes to the default calendar
    let id = ok(&h, &["add", "Dentist", "2026-11-12", "14:00"])
        .trim()
        .to_string();
    let local = ok(
        &h,
        &[
            "add",
            "-t",
            "Private",
            "-s",
            "2026-11-13 10:00",
            "--calendar",
            "",
        ],
    );
    let out = ok(&h, &["sync"]);
    assert!(
        out.contains("nc  2 events, 1 uploaded, 1 downloaded"),
        "{out}"
    );
    {
        let st = s.state.lock().unwrap();
        let mine = st.items.get(&format!("/cal/{id}.ics")).expect("uploaded");
        assert!(mine.1.contains("SUMMARY:Dentist"));
        assert_eq!(st.items.len(), 2, "the local-only event stays here");
    }
    assert_eq!(titles(&h), ["From phone", "Dentist", "Private"]);
    assert!(
        rows(&h)
            .iter()
            .any(|r| r["id"] == local.trim() && r["calendar"].is_null())
    );

    // nothing changed: no uploads, no downloads
    s.state.lock().unwrap().log.clear();
    assert!(ok(&h, &["sync"]).contains("nc  2 events\n"));
    assert_eq!(s.state.lock().unwrap().log, ["REPORT /cal/"]);

    // changed here: one PUT with If-Match
    ok(&h, &["edit", &id, "--title", "Orthodontist"]);
    assert!(ok(&h, &["sync"]).contains("1 uploaded"));
    assert!(
        s.state.lock().unwrap().items[&format!("/cal/{id}.ics")]
            .1
            .contains("SUMMARY:Orthodontist")
    );

    // changed there
    s.state.lock().unwrap().put(
        "/cal/remote.ics",
        &vevent("remote", "From phone (moved)", 11),
    );
    assert!(ok(&h, &["sync"]).contains("1 downloaded"));
    assert!(titles(&h).contains(&"From phone (moved)".to_string()));

    // changed on both sides: the server wins, with a warning
    ok(&h, &["edit", "remote", "--title", "Mine"]);
    s.state
        .lock()
        .unwrap()
        .put("/cal/remote.ics", &vevent("remote", "Theirs", 11));
    let out = vcal(&h, &["sync"]);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("\"Mine\": changed here and on the server; kept the server's"),
        "{stderr}"
    );
    assert!(titles(&h).contains(&"Theirs".to_string()));

    // deleted here → deleted there; deleted there → deleted here
    ok(&h, &["rm", &id]);
    assert!(ok(&h, &["sync"]).contains("1 deleted on the server"));
    assert!(
        !s.state
            .lock()
            .unwrap()
            .items
            .contains_key(&format!("/cal/{id}.ics"))
    );
    s.state.lock().unwrap().items.remove("/cal/remote.ics");
    assert!(ok(&h, &["sync"]).contains("1 removed here"));
    assert_eq!(titles(&h), ["Private"]);

    // moving an event in: --calendar
    ok(&h, &["edit", local.trim(), "--calendar", "nc"]);
    assert!(ok(&h, &["sync"]).contains("1 uploaded"));
    assert!(
        err(&h, &["edit", local.trim(), "--calendar", "nope"]).contains("no calendar \"nope\"")
    );

    // leaving the config keeps the events, as local ones
    let feed = h.db.with_file_name("empty.ics");
    std::fs::write(&feed, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    write_config(
        &h,
        &format!(
            "[calendars.other]\ntype = \"ics\"\nurl = \"{}\"\n",
            feed.display()
        ),
    );
    assert!(ok(&h, &["sync"]).contains("nc     no longer configured; its 1 events are local now"));
    assert!(rows(&h)[0]["calendar"].is_null());
}

#[test]
fn caldav_errors() {
    let s = server();
    let h = home();
    write_config(
        &h,
        &format!(
            "[calendars.nc]\ntype = \"caldav\"\nurl = \"{}/cal\"\nusername = \"alice\"\n\
             password_command = \"echo wrong\"\n",
            s.url
        ),
    );
    let e = err(&h, &["sync"]);
    assert!(
        e.contains("refused the username or password (HTTP 401)"),
        "{e}"
    );
    assert!(e.contains("sync failed for nc"));

    caldav_config(&h, &s.url, "read_only = true\n");
    s.state
        .lock()
        .unwrap()
        .put("/cal/x.ics", &vevent("x", "Shared", 10));
    ok(&h, &["sync"]);
    assert!(err(&h, &["rm", "x"]).contains("read-only"));
    assert!(err(&h, &["add", "A", "--calendar", "nc"]).contains("read-only"));

    write_config(
        &h,
        "[calendars.nc]\ntype = \"caldav\"\nurl = \"https://x\"\npassword_command = \"false\"\nusername = \"a\"\n",
    );
    assert!(err(&h, &["sync"]).contains("password_command failed"));
    write_config(&h, "");
    assert!(err(&h, &["sync"]).contains("no [calendars] configured"));
}
