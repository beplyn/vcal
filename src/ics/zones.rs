//! Turning a TZID into an IANA zone. Files from Outlook and Exchange use
//! Windows names ("W. Europe Standard Time") or display names ("(UTC+01:00)
//! Amsterdam, Berlin, ..."), Thunderbird used to prefix IANA names with a
//! path ("/mozilla.org/20050126_1/Europe/Berlin"), and some writers invent
//! their own and describe them only in a VTIMEZONE. In order:
//!
//! 1. an IANA name, as is or as the tail of a path
//! 2. a Windows zone name (CLDR's windowsZones, territory 001)
//! 3. the VTIMEZONE's `X-LIC-LOCATION`
//! 4. a city named in the TZID ("... Amsterdam, Berlin ...")
//! 5. an IANA zone with the VTIMEZONE's offsets (a guess; warns)
//! 6. a fixed offset from "(UTC+01:00)" or a VTIMEZONE without DST (warns)
//!
//! and failing all of those, the local zone, with a warning.

use std::cell::RefCell;
use std::collections::HashMap;

use chrono::{NaiveDate, Offset, TimeZone};
use chrono_tz::Tz;

/// CLDR windowsZones.xml, the territory-001 (default) zone of each Windows
/// name, plus a few retired names still found in old files.
const WINDOWS: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Mexico Standard Time", "America/Mexico_City"),
    ("Mexico Standard Time 2", "America/Chihuahua"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Mid-Atlantic Standard Time", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kyiv"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Armenian Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Central Asia Standard Time", "Asia/Bishkek"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("Kamchatka Standard Time", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

pub fn windows(name: &str) -> Option<Tz> {
    let name = name.trim();
    WINDOWS
        .iter()
        .find(|(w, _)| w.eq_ignore_ascii_case(name))
        .and_then(|(_, iana)| iana.parse().ok())
}

/// What a VTIMEZONE says about its zone.
#[derive(Debug, Default, Clone)]
pub struct VTimezone {
    pub lic_location: Option<String>,
    /// TZOFFSETTO and month of the latest STANDARD and DAYLIGHT parts.
    pub standard: Option<Part>,
    pub daylight: Option<Part>,
}

#[derive(Debug, Clone, Copy)]
pub struct Part {
    /// Seconds east of UTC.
    pub offset: i32,
    /// The month the part starts in (its RRULE's BYMONTH, else its DTSTART's).
    pub month: u32,
    /// DTSTART, to keep the latest of several historical parts.
    pub since: NaiveDate,
}

impl VTimezone {
    /// Offsets on 15 January and 15 July, as the VTIMEZONE's current rules have them.
    fn jan_jul(&self) -> Option<(i32, i32)> {
        let std = self.standard.or(self.daylight)?;
        let Some(dst) = self.daylight.filter(|_| self.standard.is_some()) else {
            return Some((std.offset, std.offset));
        };
        let in_dst = |m: u32| {
            if dst.month < std.month {
                dst.month <= m && m < std.month
            } else {
                m >= dst.month || m < std.month
            }
        };
        let at = |m| if in_dst(m) { dst.offset } else { std.offset };
        Some((at(1), at(7)))
    }
}

/// `+0100`, `-0530`, `+013000` as seconds east of UTC.
pub fn parse_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    let (sign, digits) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => return None,
    };
    if !(digits.len() == 4 || digits.len() == 6) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| digits.get(r).and_then(|d| d.parse::<i32>().ok());
    let secs = n(0..2)? * 3600 + n(2..4)? * 60 + n(4..6).unwrap_or(0);
    Some(sign * secs)
}

/// The zone for a fixed offset, if there is one (Etc/GMT zones are whole
/// hours, and their sign is inverted).
fn fixed(offset: i32) -> Option<Tz> {
    if offset == 0 {
        return Some(Tz::UTC);
    }
    if offset % 3600 != 0 {
        return None;
    }
    let hours = -offset / 3600;
    format!("Etc/GMT{hours:+}").parse().ok()
}

/// `(UTC+01:00) ...` → seconds east of UTC.
fn display_offset(tzid: &str) -> Option<i32> {
    let rest = tzid
        .trim()
        .strip_prefix("(UTC")
        .or_else(|| tzid.trim().strip_prefix("(GMT"))?;
    if rest.starts_with(')') {
        return Some(0);
    }
    let end = rest.find(')')?;
    parse_offset(&rest[..end].replace(':', ""))
}

/// The tail of a path that names an IANA zone: `/mozilla.org/x/Europe/Berlin`.
fn path_tail(tzid: &str) -> Option<Tz> {
    let parts: Vec<&str> = tzid.split('/').filter(|p| !p.is_empty()).collect();
    (1..parts.len())
        .map(|i| parts[i..].join("/"))
        .find_map(|tail| tail.parse().ok())
}

/// An IANA zone whose city appears in the TZID as a word.
fn city(tzid: &str) -> Option<Tz> {
    let words: Vec<String> = tzid
        .split([',', '(', ')', '&', ';'])
        .map(|w| w.trim().replace(' ', "_").to_ascii_lowercase())
        .filter(|w| w.len() > 2)
        .collect();
    if words.is_empty() {
        return None;
    }
    // Canonical zones only: Area/City, not Etc/ or legacy aliases like US/...
    chrono_tz::TZ_VARIANTS.iter().copied().find(|tz| {
        let name = tz.name();
        let Some((area, city)) = name.rsplit_once('/') else {
            return false;
        };
        let area = area.split('/').next().unwrap_or(area);
        [
            "Africa",
            "America",
            "Antarctica",
            "Asia",
            "Atlantic",
            "Australia",
            "Europe",
            "Indian",
            "Pacific",
        ]
        .contains(&area)
            && words.iter().any(|w| w == &city.to_ascii_lowercase())
    })
}

fn offsets_in(tz: Tz, year: i32) -> (i32, i32) {
    let at = |m| {
        let d = NaiveDate::from_ymd_opt(year, m, 15).unwrap_or_default();
        tz.offset_from_utc_date(&d).fix().local_minus_utc()
    };
    (at(1), at(7))
}

/// A zone with the VTIMEZONE's January and July offsets, preferring the
/// zones the Windows table names (one well-known zone per region).
fn by_offsets(vtz: &VTimezone, year: i32) -> Option<Tz> {
    let want = vtz.jan_jul()?;
    WINDOWS
        .iter()
        .filter_map(|(_, iana)| iana.parse::<Tz>().ok())
        .filter(|tz| !tz.name().starts_with("Etc/"))
        .chain(chrono_tz::TZ_VARIANTS.iter().copied())
        .find(|tz| offsets_in(*tz, year) == want)
}

/// How a TZID was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Unambiguous: an IANA name, a Windows name, X-LIC-LOCATION, a city.
    Known(Tz),
    /// A zone with the right offsets, or a fixed offset; worth a warning.
    Guessed(Tz),
    Unknown,
}

pub fn resolve(tzid: &str, vtz: Option<&VTimezone>, year: i32) -> Resolution {
    let tzid = tzid.trim();
    if let Ok(tz) = tzid.parse::<Tz>() {
        return Resolution::Known(tz);
    }
    if let Some(tz) = path_tail(tzid).or_else(|| windows(tzid)) {
        return Resolution::Known(tz);
    }
    if let Some(tz) = vtz
        .and_then(|v| v.lic_location.as_deref())
        .and_then(|l| l.trim().parse().ok())
    {
        return Resolution::Known(tz);
    }
    if let Some(tz) = city(tzid) {
        return Resolution::Known(tz);
    }
    if let Some(tz) = vtz.and_then(|v| by_offsets(v, year)) {
        return Resolution::Guessed(tz);
    }
    if let Some(tz) = display_offset(tzid).and_then(fixed) {
        return Resolution::Guessed(tz);
    }
    Resolution::Unknown
}

/// Per-file TZID lookups: resolves each name once and collects a warning
/// for every guess.
pub struct Zones {
    pub local: Tz,
    vtimezones: HashMap<String, VTimezone>,
    year: i32,
    cache: RefCell<HashMap<String, Tz>>,
    pub warnings: RefCell<Vec<String>>,
}

impl Zones {
    pub fn new(local: Tz, vtimezones: HashMap<String, VTimezone>, year: i32) -> Self {
        Zones {
            local,
            vtimezones,
            year,
            cache: RefCell::default(),
            warnings: RefCell::default(),
        }
    }

    pub fn get(&self, tzid: &str) -> Tz {
        if let Some(tz) = self.cache.borrow().get(tzid) {
            return *tz;
        }
        let tz = match resolve(tzid, self.vtimezones.get(tzid), self.year) {
            Resolution::Known(tz) => tz,
            Resolution::Guessed(tz) => {
                self.warnings.borrow_mut().push(format!(
                    "time zone {tzid:?} isn't one vcal knows; read as {} (same offsets)",
                    tz.name()
                ));
                tz
            }
            Resolution::Unknown => {
                self.warnings.borrow_mut().push(format!(
                    "time zone {tzid:?} isn't one vcal knows; read as local time ({})",
                    self.local.name()
                ));
                self.local
            }
        };
        self.cache.borrow_mut().insert(tzid.to_string(), tz);
        tz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_windows_zone_exists() {
        for (w, iana) in WINDOWS {
            assert!(iana.parse::<Tz>().is_ok(), "{w} -> {iana}");
        }
    }

    fn known(tzid: &str) -> Option<&'static str> {
        match resolve(tzid, None, 2026) {
            Resolution::Known(tz) => Some(tz.name()),
            _ => None,
        }
    }

    #[test]
    fn names() {
        assert_eq!(known("Europe/Berlin"), Some("Europe/Berlin"));
        assert_eq!(known("W. Europe Standard Time"), Some("Europe/Berlin"));
        assert_eq!(known("eastern standard time"), Some("America/New_York"));
        assert_eq!(
            known("/mozilla.org/20050126_1/America/Los_Angeles"),
            Some("America/Los_Angeles")
        );
        assert_eq!(
            known("/freeassociation.sourceforge.net/Tzfile/Europe/London"),
            Some("Europe/London")
        );
        assert_eq!(
            known("(UTC+01:00) Amsterdam, Berlin, Bern, Rome, Stockholm, Vienna"),
            Some("Europe/Amsterdam")
        );
        assert_eq!(known("Sometime Standard Time"), None);
    }

    #[test]
    fn vtimezone_fallbacks() {
        let lic = VTimezone {
            lic_location: Some("Europe/Vienna".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve("Custom", Some(&lic), 2026),
            Resolution::Known(chrono_tz::Europe::Vienna)
        );

        let day = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        let cet = VTimezone {
            lic_location: None,
            standard: Some(Part {
                offset: 3600,
                month: 10,
                since: day,
            }),
            daylight: Some(Part {
                offset: 7200,
                month: 3,
                since: day,
            }),
        };
        let Resolution::Guessed(tz) = resolve("Zone 1", Some(&cet), 2026) else {
            panic!("expected a guess");
        };
        assert_eq!(offsets_in(tz, 2026), (3600, 7200));

        // southern hemisphere: summer time over January
        let aest = VTimezone {
            lic_location: None,
            standard: Some(Part {
                offset: 36000,
                month: 4,
                since: day,
            }),
            daylight: Some(Part {
                offset: 39600,
                month: 10,
                since: day,
            }),
        };
        let Resolution::Guessed(tz) = resolve("Zone 2", Some(&aest), 2026) else {
            panic!("expected a guess");
        };
        assert_eq!(offsets_in(tz, 2026), (39600, 36000));

        assert_eq!(
            resolve("(UTC+05:00) Somewhere", None, 2026),
            Resolution::Guessed("Etc/GMT-5".parse().unwrap())
        );
    }

    #[test]
    fn offsets() {
        assert_eq!(parse_offset("+0100"), Some(3600));
        assert_eq!(parse_offset("-0530"), Some(-19800));
        assert_eq!(parse_offset("+013015"), Some(5415));
        assert_eq!(parse_offset("0100"), None);
    }
}
