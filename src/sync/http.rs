//! A small blocking HTTP client over ureq: any method (CalDAV needs REPORT
//! and PROPFIND), Basic auth, and statuses returned rather than raised.

use std::time::Duration;

use anyhow::{Context, Result};

pub struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub body: String,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub struct Client {
    agent: ureq::Agent,
    auth: Option<String>,
}

/// A URL fit for messages: no query string or credentials, which for share
/// links (Proton's) are the secret.
pub fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    let more = if path.len() < rest.len() { "?…" } else { "" };
    if scheme.is_empty() {
        format!("{path}{more}")
    } else {
        format!("{scheme}://{path}{more}")
    }
}

fn base64(input: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Client {
    pub fn new(credentials: Option<(&str, &str)>) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("vcal/", env!("CARGO_PKG_VERSION")))
            .build();
        Client {
            agent: ureq::Agent::new_with_config(config),
            auth: credentials
                .map(|(u, p)| format!("Basic {}", base64(format!("{u}:{p}").as_bytes()))),
        }
    }

    pub fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<Response> {
        let mut req = ureq::http::Request::builder().method(method).uri(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let shown = redact(url);
        let result = match body {
            Some(b) => self.agent.run(
                req.body(b.to_string())
                    .with_context(|| format!("bad request to {shown}"))?,
            ),
            None => self.agent.run(
                req.body(())
                    .with_context(|| format!("bad request to {shown}"))?,
            ),
        };
        let mut resp = result.map_err(|e| anyhow::anyhow!("{method} {shown}: {e}"))?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let (etag, last_modified) = (header("etag"), header("last-modified"));
        let status = resp.status().as_u16();
        let body = resp
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .lossy_utf8(true)
            .read_to_string()
            .with_context(|| format!("{method} {shown}: reading the response"))?;
        Ok(Response {
            status,
            etag,
            last_modified,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        for (plain, coded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("alice:pä55", "YWxpY2U6cMOkNTU="),
        ] {
            assert_eq!(base64(plain.as_bytes()), coded, "{plain}");
        }
    }

    #[test]
    fn redacts_secrets() {
        assert_eq!(
            redact(
                "https://calendar.proton.me/api/calendar/v1/url/abc/calendar.ics?CacheKey=x&PassphraseKey=y"
            ),
            "https://calendar.proton.me/api/calendar/v1/url/abc/calendar.ics?…"
        );
        assert_eq!(redact("https://u:p@host/cal/"), "https://host/cal/");
        assert_eq!(redact("/srv/x.ics"), "/srv/x.ics");
    }
}
