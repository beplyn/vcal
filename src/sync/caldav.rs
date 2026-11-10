//! Two-way sync with one CalDAV collection (RFC 4791), as Nextcloud,
//! Radicale and Baïkal serve them. Each event is one resource holding the
//! series and its RECURRENCE-ID overrides.
//!
//! A sync pushes first, then pulls:
//!
//! 1. events deleted here (a sync row without its event): DELETE, If-Match
//! 2. events created here (no sync row): PUT, If-None-Match: *
//! 3. events changed here (`updated_ts` past the row's): PUT, If-Match
//! 4. list the collection's ETags; fetch what's new or changed
//!    (calendar-multiget) and store it; drop what the server no longer has
//!
//! A precondition failure (412) means the server changed it too: the
//! server's version wins at step 4, with a warning naming the event.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono_tz::Tz;

use super::Done;
use super::http::{self, Client, Response};
use crate::config::CalendarConfig;
use crate::db::Db;
use crate::ics;
use crate::time;

const DAV: &str = "DAV:";
const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";

const LIST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop><d:getetag/></d:prop>
  <c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"/></c:comp-filter></c:filter>
</c:calendar-query>"#;

/// How many resources one calendar-multiget asks for.
const BATCH: usize = 50;

/// One `<d:response>` of a multistatus.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub href: String,
    pub etag: Option<String>,
    pub data: Option<String>,
    /// The response's or its successful propstat's status code.
    pub status: Option<u16>,
}

fn status_code(text: &str) -> Option<u16> {
    // "HTTP/1.1 200 OK"
    text.split_whitespace().nth(1)?.parse().ok()
}

/// Reads a 207 Multi-Status body.
pub fn multistatus(xml: &str) -> Result<Vec<Entry>> {
    let doc = roxmltree::Document::parse(xml).context("the server sent XML vcal can't read")?;
    let is = |n: &roxmltree::Node, ns: &str, name: &str| {
        n.is_element() && n.tag_name().name() == name && n.tag_name().namespace() == Some(ns)
    };
    let mut out = Vec::new();
    for resp in doc.descendants().filter(|n| is(n, DAV, "response")) {
        let mut e = Entry::default();
        for child in resp.children() {
            if is(&child, DAV, "href") {
                e.href = child.text().unwrap_or("").trim().to_string();
            } else if is(&child, DAV, "status") {
                e.status = child.text().and_then(status_code);
            } else if is(&child, DAV, "propstat") {
                let ok = child
                    .children()
                    .find(|c| is(c, DAV, "status"))
                    .and_then(|s| s.text())
                    .and_then(status_code)
                    .is_none_or(|s| (200..300).contains(&s));
                if !ok {
                    continue;
                }
                e.status.get_or_insert(200);
                for prop in child.children().filter(|c| is(c, DAV, "prop")) {
                    for p in prop.children() {
                        if is(&p, DAV, "getetag") {
                            e.etag = p
                                .text()
                                .map(|t| t.trim().to_string())
                                .filter(|t| !t.is_empty());
                        } else if is(&p, CALDAV, "calendar-data") {
                            e.data = p.text().map(str::to_string);
                        }
                    }
                }
            }
        }
        if !e.href.is_empty() {
            out.push(e);
        }
    }
    Ok(out)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The scheme and authority of a URL: `https://host:port`.
fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    match url[after..].find('/') {
        Some(i) => &url[..after + i],
        None => url,
    }
}

/// The path of a URL: `/remote.php/dav/calendars/a/personal/`.
fn path(url: &str) -> &str {
    &url[origin(url).len()..]
}

/// An href from the server as a URL to send requests to.
fn absolute(collection: &str, href: &str) -> String {
    if href.contains("://") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("{}{href}", origin(collection))
    } else {
        format!("{collection}{href}")
    }
}

/// A resource name for a new event: its id with anything unusual replaced.
fn resource_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{safe}.ics")
}

/// Percent-decoding, for comparing hrefs written differently.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(h, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn check(resp: &Response, what: &str) -> Result<()> {
    match resp.status {
        s if (200..300).contains(&s) => Ok(()),
        401 => bail!("{what}: the server refused the username or password (HTTP 401)"),
        403 => bail!("{what}: not allowed (HTTP 403)"),
        404 => bail!("{what}: no calendar there (HTTP 404); check the url"),
        s => bail!("{what}: HTTP {s}"),
    }
}

pub fn sync(db: &mut Db, cal: &CalendarConfig, client: &Client, tz: Tz) -> Result<Done> {
    let base = &cal.url;
    let shown = http::redact(base);
    let mut warnings = Vec::new();
    let mut conflicts: HashSet<String> = HashSet::new();
    let (mut pushed, mut deleted_there) = (0, 0);

    let local = db.events_in(&cal.name)?;
    let local_ids: HashSet<&str> = local.iter().map(|e| e.id.as_str()).collect();
    let items: HashMap<String, crate::db::SyncItem> = db
        .sync_items(&cal.name)?
        .into_iter()
        .map(|i| (i.event_id.clone(), i))
        .collect();

    if !cal.read_only {
        // 1. deleted here
        for item in items
            .values()
            .filter(|i| !local_ids.contains(i.event_id.as_str()))
        {
            let mut headers = vec![];
            if let Some(e) = item.etag.as_deref() {
                headers.push(("If-Match", e));
            }
            let resp = client.send("DELETE", &absolute(base, &item.href), &headers, None)?;
            match resp.status {
                412 => warnings.push(format!(
                    "{}: deleted here but changed on the server; kept the server's",
                    item.href
                )),
                404 => {}
                _ => {
                    check(&resp, &format!("deleting {}", item.href))?;
                    deleted_there += 1;
                }
            }
            db.forget_item(&cal.name, &item.event_id)?;
        }

        // 2 and 3. created or changed here
        let now = time::now();
        for ev in &local {
            let item = items.get(&ev.id);
            if item.is_some_and(|i| ev.updated <= i.synced) {
                continue;
            }
            let href = match item {
                Some(i) => i.href.clone(),
                None => format!("{}{}", path(base), resource_name(&ev.id)),
            };
            let body = ics::write::calendar(&[(ev.clone(), db.exceptions(&ev.id)?)], now);
            let mut headers = vec![("Content-Type", "text/calendar; charset=utf-8")];
            match item.and_then(|i| i.etag.as_deref()) {
                Some(e) => headers.push(("If-Match", e)),
                None if item.is_none() => headers.push(("If-None-Match", "*")),
                None => {}
            }
            let resp = client.send("PUT", &absolute(base, &href), &headers, Some(&body))?;
            if resp.status == 412 {
                conflicts.insert(ev.id.clone());
                warnings.push(format!(
                    "\"{}\": changed here and on the server; kept the server's",
                    ev.title
                ));
                continue;
            }
            check(&resp, &format!("uploading \"{}\"", ev.title))?;
            db.mark_pushed(&cal.name, &ev.id, &href, resp.etag.as_deref())?;
            pushed += 1;
        }
    }

    // 4. what the server has
    let resp = client.send(
        "REPORT",
        base,
        &[
            ("Depth", "1"),
            ("Content-Type", "application/xml; charset=utf-8"),
        ],
        Some(LIST),
    )?;
    check(&resp, &format!("listing {shown}"))?;
    let collection = decode(path(base));
    let remote: Vec<Entry> = multistatus(&resp.body)?
        .into_iter()
        .filter(|e| decode(&e.href).trim_end_matches('/') != collection.trim_end_matches('/'))
        .filter(|e| e.status.is_none_or(|s| (200..300).contains(&s)))
        .collect();

    let items = db.sync_items(&cal.name)?;
    let by_href: HashMap<String, &crate::db::SyncItem> =
        items.iter().map(|i| (decode(&i.href), i)).collect();
    let wanted: Vec<&Entry> = remote
        .iter()
        .filter(|e| {
            let known = by_href.get(&decode(&e.href));
            let conflicted = known.is_some_and(|i| conflicts.contains(&i.event_id));
            conflicted || known.is_none_or(|i| e.etag.is_none() || i.etag != e.etag)
        })
        .collect();

    let mut pulled = 0;
    for chunk in wanted.chunks(BATCH) {
        let hrefs: String = chunk
            .iter()
            .map(|e| format!("  <d:href>{}</d:href>\n", escape_xml(&e.href)))
            .collect();
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<c:calendar-multiget xmlns:d=\"DAV:\" \
             xmlns:c=\"urn:ietf:params:xml:ns:caldav\">\n  <d:prop><d:getetag/><c:calendar-data/></d:prop>\n\
             {hrefs}</c:calendar-multiget>"
        );
        let resp = client.send(
            "REPORT",
            base,
            &[
                ("Depth", "1"),
                ("Content-Type", "application/xml; charset=utf-8"),
            ],
            Some(&body),
        )?;
        check(&resp, &format!("fetching events from {shown}"))?;
        for e in multistatus(&resp.body)? {
            let Some(data) = e.data.as_deref() else {
                continue;
            };
            let parsed = match ics::parse(data, tz) {
                Ok(p) => p,
                Err(err) => {
                    warnings.push(format!("{}: {err:#}", e.href));
                    continue;
                }
            };
            warnings.extend(
                parsed
                    .warnings
                    .into_iter()
                    .map(|w| format!("{}: {w}", e.href)),
            );
            let mut events = parsed.events.into_iter();
            let Some(ev) = events.next() else {
                continue;
            };
            if events.next().is_some() {
                warnings.push(format!(
                    "{}: holds more than one event; vcal keeps the first",
                    e.href
                ));
            }
            let mut exceptions = parsed.exceptions;
            exceptions.retain(|x| x.event_id == ev.id);
            db.store_remote(&cal.name, &e.href, e.etag.as_deref(), ev, exceptions)?;
            pulled += 1;
        }
    }

    // gone from the server
    let present: HashSet<String> = remote.iter().map(|e| decode(&e.href)).collect();
    let mut removed = 0;
    for item in db.sync_items(&cal.name)? {
        if !present.contains(&decode(&item.href)) {
            db.delete_remote(&cal.name, &item.event_id)?;
            removed += 1;
        }
    }

    let mut parts = vec![format!("{} events", remote.len())];
    for (n, what) in [
        (pushed, "uploaded"),
        (deleted_there, "deleted on the server"),
        (pulled, "downloaded"),
        (removed, "removed here"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    Ok(Done {
        summary: parts.join(", "),
        warnings,
        etag: None,
        modified: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_multistatus() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
 <d:response>
  <d:href>/dav/cal/a.ics</d:href>
  <d:propstat><d:prop><d:getetag>"1"</d:getetag><cal:calendar-data>BEGIN:VCALENDAR
END:VCALENDAR</cal:calendar-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
 </d:response>
 <d:response><d:href>/dav/cal/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>
 <response xmlns="DAV:"><href>/dav/cal/b%20c.ics</href>
  <propstat><prop><getetag>W/"2"</getetag></prop><status>HTTP/1.1 200 OK</status></propstat>
  <propstat><prop><displayname/></prop><status>HTTP/1.1 404 Not Found</status></propstat>
 </response>
</d:multistatus>"#;
        let got = multistatus(xml).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].etag.as_deref(), Some("\"1\""));
        assert!(
            got[0]
                .data
                .as_deref()
                .unwrap()
                .starts_with("BEGIN:VCALENDAR")
        );
        assert_eq!(got[1].status, Some(404));
        assert_eq!(
            (got[2].href.as_str(), got[2].etag.as_deref()),
            ("/dav/cal/b%20c.ics", Some("W/\"2\""))
        );
    }

    #[test]
    fn urls() {
        let c = "https://cloud.example.com:8443/remote.php/dav/calendars/a/p/";
        assert_eq!(origin(c), "https://cloud.example.com:8443");
        assert_eq!(path(c), "/remote.php/dav/calendars/a/p/");
        assert_eq!(
            absolute(c, "/remote.php/dav/calendars/a/p/x.ics"),
            "https://cloud.example.com:8443/remote.php/dav/calendars/a/p/x.ics"
        );
        assert_eq!(absolute(c, "x.ics"), format!("{c}x.ics"));
        assert_eq!(resource_name("abc@example.com/1"), "abc-example.com-1.ics");
        assert_eq!(decode("/a%20b%2Fc"), "/a b/c");
    }
}
