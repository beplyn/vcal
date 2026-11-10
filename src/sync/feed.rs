//! Read-only iCalendar feeds: an http(s)/webcal URL, a Proton Calendar
//! share link, or a file. Fetched with If-None-Match / If-Modified-Since,
//! and on a change the calendar's events become exactly the feed's.

use anyhow::{Context, Result, bail};
use chrono_tz::Tz;

use super::Done;
use super::http::{self, Client};
use crate::config::{CalendarConfig, CalendarKind};
use crate::db::Db;
use crate::ics;

/// Feed events get ids under the calendar's name, so the same UID in two
/// feeds, or in a file you imported, stays two events.
pub fn event_id(calendar: &str, uid: &str) -> String {
    format!("{calendar}:{uid}")
}

fn web_url(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(url.to_string())
    } else if let Some(rest) = lower
        .strip_prefix("webcals://")
        .or_else(|| lower.strip_prefix("webcal://"))
    {
        // the scheme only; keep the rest's case (tokens are case-sensitive)
        let offset = url.len() - rest.len();
        Some(format!("https://{}", &url[offset..]))
    } else {
        None
    }
}

pub fn sync(db: &mut Db, cal: &CalendarConfig, tz: Tz) -> Result<Done> {
    let mut done = Done::default();
    let text = match web_url(&cal.url) {
        None => {
            std::fs::read_to_string(&cal.url).with_context(|| format!("can't read {}", cal.url))?
        }
        Some(url) => {
            let known = db.calendar(&cal.name)?;
            let mut headers = vec![("Accept", "text/calendar, */*;q=0.5")];
            let (etag, modified) = known
                .as_ref()
                .map(|k| (k.etag.clone(), k.modified.clone()))
                .unwrap_or_default();
            // only trust caching headers when the events are still there
            let has_events = known.as_ref().is_some_and(|k| k.events > 0);
            if has_events {
                if let Some(e) = etag.as_deref() {
                    headers.push(("If-None-Match", e));
                }
                if let Some(m) = modified.as_deref() {
                    headers.push(("If-Modified-Since", m));
                }
            }
            let resp = Client::new(None).send("GET", &url, &headers, None)?;
            match resp.status {
                304 => {
                    done.summary = format!("unchanged ({} events)", known.map_or(0, |k| k.events));
                    return Ok(done);
                }
                401 | 403 | 404 if cal.kind == CalendarKind::Proton => bail!(
                    "HTTP {} from {}: the share link may have been revoked or changed in Proton \
                     Calendar (Share → link)",
                    resp.status,
                    http::redact(&url)
                ),
                s if !resp.ok() => bail!("HTTP {s} from {}", http::redact(&url)),
                _ => {}
            }
            done.etag = resp.etag;
            done.modified = resp.last_modified;
            resp.body
        }
    };

    let mut parsed = ics::parse(&text, tz)
        .with_context(|| format!("{} isn't an iCalendar feed", http::redact(&cal.url)))?;
    for ev in &mut parsed.events {
        ev.id = event_id(&cal.name, &ev.id);
        if ev.category.is_none() {
            ev.category = cal.category.clone();
        }
    }
    for x in &mut parsed.exceptions {
        x.event_id = event_id(&cal.name, &x.event_id);
    }
    let total = parsed.events.len();
    let (new, removed) = db.replace_calendar(&cal.name, parsed.events, parsed.exceptions)?;
    done.summary = format!("{total} events ({new} new, {removed} removed)");
    done.warnings = parsed.warnings;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webcal_becomes_https() {
        assert_eq!(
            web_url("webcal://Example.com/Cal.ics?Token=AbC").as_deref(),
            Some("https://Example.com/Cal.ics?Token=AbC")
        );
        assert_eq!(web_url("WEBCALS://x/y").as_deref(), Some("https://x/y"));
        assert_eq!(web_url("http://x/y").as_deref(), Some("http://x/y"));
        assert_eq!(web_url("/srv/x.ics"), None);
    }
}
