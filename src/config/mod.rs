//! config.toml: parsing, validation, and writing the documented default.
//!
//! Lookup: `--config` / `$VCAL_CONFIG`, else
//! `$XDG_CONFIG_HOME/calendar/config.toml`, else `~/.config/calendar/config.toml`.
//! Every key is optional; anything missing keeps its built-in default.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::Weekday;
use serde::Deserialize;

use crate::settings::{Settings, View};
use crate::time;
use crate::tui::keymap::{Action, KeyMode, Keymap, parse_keys};
use crate::tui::theme::{self, Theme};

/// The file written when none exists; also documents every option.
pub const DEFAULT_TOML: &str = include_str!("default.toml");

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub settings: Settings,
    pub theme: Theme,
    pub keymap: Keymap,
    /// `[general] database_path`, `~` expanded.
    pub database_path: Option<PathBuf>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Raw {
    general: General,
    views: Views,
    theme: BTreeMap<String, toml::Value>,
    keybindings: Keybindings,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct General {
    week_start: Option<String>,
    clock: Option<String>,
    default_duration: Option<DurationValue>,
    database_path: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DurationValue {
    Minutes(i64),
    Text(String),
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Views {
    start: Option<String>,
    slot_minutes: Option<u32>,
    first_hour: Option<u32>,
    last_hour: Option<u32>,
    new_event_hour: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Keybindings {
    normal: BTreeMap<String, Vec<String>>,
    visual: BTreeMap<String, Vec<String>>,
}

pub fn default_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("calendar")
        .join("config.toml")
}

/// Expands a leading `~/`; the result must be absolute.
fn expand_home(s: &str) -> Result<PathBuf> {
    let path = match s.strip_prefix("~/") {
        Some(rest) => {
            PathBuf::from(std::env::var_os("HOME").context("$HOME isn't set")?).join(rest)
        }
        None => PathBuf::from(s),
    };
    ensure!(
        path.is_absolute(),
        "{s:?} must be absolute or start with ~/"
    );
    Ok(path)
}

/// Parses and validates config text. Errors name the offending key.
pub fn parse(text: &str) -> Result<Config> {
    let raw: Raw = toml::from_str(text).map_err(|e| anyhow!("{}", e.to_string().trim_end()))?;
    let mut cfg = Config::default();
    let s = &mut cfg.settings;

    let g = raw.general;
    if let Some(w) = g.week_start {
        s.week_start = w
            .parse::<Weekday>()
            .map_err(|_| anyhow!("[general] week_start: {w:?} isn't a weekday"))?;
    }
    if let Some(c) = g.clock {
        s.clock_24h = match c.to_ascii_lowercase().as_str() {
            "24h" | "24" => true,
            "12h" | "12" => false,
            _ => bail!("[general] clock: {c:?} must be \"24h\" or \"12h\""),
        };
    }
    if let Some(d) = g.default_duration {
        s.default_duration = match d {
            DurationValue::Minutes(m) => m * 60,
            DurationValue::Text(t) => {
                time::parse_duration(&t).context("[general] default_duration")?
            }
        };
        ensure!(
            s.default_duration > 0,
            "[general] default_duration must be positive"
        );
    }
    if let Some(p) = g.database_path {
        cfg.database_path = Some(expand_home(&p).context("[general] database_path")?);
    }

    let v = raw.views;
    if let Some(start) = v.start {
        s.start_view = View::parse(&start).ok_or_else(|| {
            anyhow!("[views] start: {start:?} must be month, week, day or agenda")
        })?;
    }
    if let Some(m) = v.slot_minutes {
        ensure!(
            [15, 30, 60].contains(&m),
            "[views] slot_minutes: {m} must be 15, 30 or 60"
        );
        s.slot_minutes = m;
    }
    s.first_hour = v.first_hour.unwrap_or(s.first_hour);
    s.last_hour = v.last_hour.unwrap_or(s.last_hour);
    ensure!(
        s.first_hour < s.last_hour && s.last_hour <= 24,
        "[views] first_hour/last_hour: need 0 <= first < last <= 24 (got {} and {})",
        s.first_hour,
        s.last_hour
    );
    if let Some(h) = v.new_event_hour {
        ensure!(h < 24, "[views] new_event_hour: {h} isn't an hour (0-23)");
        s.default_start_hour = h;
    }

    for (key, value) in raw.theme {
        if key == "roles" {
            let table = value
                .as_table()
                .ok_or_else(|| anyhow!("[theme.roles] must be a table"))?;
            for (role, slot) in table {
                let slot = slot.as_str().ok_or_else(|| {
                    anyhow!("[theme.roles] {role}: expected a string like \"base0D\"")
                })?;
                let slot =
                    theme::parse_slot(slot).with_context(|| format!("[theme.roles] {role}"))?;
                cfg.theme.roles.set(role, slot).context("[theme.roles]")?;
            }
            continue;
        }
        let slot = theme::parse_slot(&key).map_err(|_| {
            anyhow!("[theme] unknown key {key:?} (base00 .. base0F, or a [theme.roles] table)")
        })?;
        let hex = value
            .as_str()
            .ok_or_else(|| anyhow!("[theme] {key}: expected \"#rrggbb\""))?;
        cfg.theme.base[slot] = theme::parse_hex(hex).with_context(|| format!("[theme] {key}"))?;
    }

    for (mode, table, section) in [
        (
            KeyMode::Normal,
            raw.keybindings.normal,
            "[keybindings.normal]",
        ),
        (
            KeyMode::Visual,
            raw.keybindings.visual,
            "[keybindings.visual]",
        ),
    ] {
        for (name, sequences) in table {
            let action: Action = name.parse().map_err(|_| {
                let names: Vec<_> = Action::ALL.iter().map(|a| a.name()).collect();
                anyhow!(
                    "{section} unknown action {name:?} (one of: {})",
                    names.join(", ")
                )
            })?;
            let keys = sequences
                .iter()
                .map(|s| parse_keys(s).with_context(|| format!("{section} {name}: {s:?}")))
                .collect::<Result<Vec<_>>>()?;
            cfg.keymap.rebind(mode, action, keys);
        }
    }
    let conflicts = cfg.keymap.conflicts();
    ensure!(
        conflicts.is_empty(),
        "[keybindings] bindings that could never fire (a prefix of another, or starting \
         with a count digit): {}",
        conflicts.join("; ")
    );
    Ok(cfg)
}

pub fn load(path: &Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?;
    parse(&text).with_context(|| format!("{}", path.display()))
}

/// Loads `path`, first writing the documented default if nothing is there.
/// Returns a note for the user when a file was written or couldn't be.
pub fn load_or_create(path: &Path) -> Result<(Config, Option<String>)> {
    if path.exists() || path.is_symlink() {
        return Ok((load(path)?, None));
    }
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, DEFAULT_TOML));
    let note = match written {
        Ok(()) => format!("wrote the default config to {}", path.display()),
        Err(e) => format!("using defaults; couldn't write {}: {e}", path.display()),
    };
    Ok((Config::default(), Some(note)))
}

/// What identifies a version of the file: the resolved target (Home-Manager
/// swaps a symlink), mtime and size. `None` when there's no file.
pub type Stamp = Option<(PathBuf, SystemTime, u64)>;

pub fn stamp(path: &Path) -> Stamp {
    let real = std::fs::canonicalize(path).ok()?;
    let meta = std::fs::metadata(&real).ok()?;
    Some((real, meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::KeyMode;

    fn bindings(km: &Keymap) -> Vec<(KeyMode, &'static str, Vec<String>)> {
        let mut out = Vec::new();
        for mode in [KeyMode::Normal, KeyMode::Visual] {
            for a in Action::ALL {
                let mut keys = km.keys_for(mode, *a);
                keys.sort();
                out.push((mode, a.name(), keys));
            }
        }
        out
    }

    #[test]
    fn default_file_is_the_defaults() {
        let cfg = parse(DEFAULT_TOML).unwrap();
        assert_eq!(cfg.settings, Settings::default());
        assert_eq!(cfg.theme, Theme::default());
        assert_eq!(bindings(&cfg.keymap), bindings(&Keymap::default()));
        assert_eq!(cfg.database_path, None);
    }

    #[test]
    fn commented_examples_are_accurate() {
        // Uncommenting every `# key = value` line must change nothing.
        let text: String = DEFAULT_TOML
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(rest)
                    if rest.contains(" = ")
                        && rest.starts_with(|c: char| c.is_ascii_lowercase()) =>
                {
                    rest
                }
                _ => l,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("\nquit = [\"ZZ\"]"),
            "the uncommenting worked"
        );
        let cfg = parse(&text).unwrap();
        assert_eq!(cfg.settings, Settings::default());
        assert_eq!(cfg.theme, Theme::default());
        assert_eq!(bindings(&cfg.keymap), bindings(&Keymap::default()));
        assert!(
            cfg.database_path
                .unwrap()
                .ends_with(".local/share/calendar/events.db")
        );
    }

    #[test]
    fn every_action_is_documented() {
        for a in Action::ALL {
            assert!(
                DEFAULT_TOML.contains(&format!("# {} = [", a.name())),
                "{}",
                a.name()
            );
        }
        for r in theme::Roles::NAMES {
            assert!(DEFAULT_TOML.contains(&format!("# {r} = \"base")), "{r}");
        }
    }

    #[test]
    fn empty_file_is_fine() {
        assert_eq!(parse("").unwrap().settings, Settings::default());
    }

    #[test]
    fn reads_values() {
        let cfg = parse(
            r##"
            [general]
            week_start = "sunday"
            clock = "12h"
            default_duration = 45
            database_path = "/srv/cal.db"
            [views]
            start = "week"
            slot_minutes = 15
            first_hour = 7
            last_hour = 22
            [theme]
            base0D = "#112233"
            [theme.roles]
            accent = "base0E"
            [keybindings.normal]
            quit = ["q", "ZZ"]
            help = []
            "##,
        )
        .unwrap();
        let s = &cfg.settings;
        assert_eq!(s.week_start, Weekday::Sun);
        assert!(!s.clock_24h);
        assert_eq!(s.default_duration, 45 * 60);
        assert_eq!(
            (s.start_view, s.slot_minutes, s.first_hour, s.last_hour),
            (View::Week, 15, 7, 22)
        );
        assert_eq!(cfg.database_path.as_deref(), Some(Path::new("/srv/cal.db")));
        assert_eq!(
            cfg.theme.base[0xD],
            ratatui::style::Color::Rgb(0x11, 0x22, 0x33)
        );
        assert_eq!(cfg.theme.accent(), cfg.theme.base[0xE]);
        assert_eq!(
            cfg.keymap.keys_for(KeyMode::Normal, Action::Quit),
            ["q", "ZZ"]
        );
        assert!(
            cfg.keymap
                .keys_for(KeyMode::Normal, Action::Help)
                .is_empty()
        );
    }

    #[test]
    fn errors_name_the_key() {
        for (text, needle) in [
            ("[general]\nclock = \"25h\"", "[general] clock"),
            ("[general]\nweek_start = \"funday\"", "week_start"),
            ("[general]\ndefault_duration = \"soon\"", "default_duration"),
            ("[general]\ndatabase_path = \"rel/x.db\"", "database_path"),
            ("[views]\nslot_minutes = 20", "slot_minutes"),
            (
                "[views]\nfirst_hour = 9\nlast_hour = 9",
                "first_hour/last_hour",
            ),
            ("[views]\nstart = \"year\"", "[views] start"),
            ("[theme]\nbase0D = \"blue\"", "[theme] base0D"),
            ("[theme]\nbase0G = \"#000000\"", "unknown key"),
            ("[theme.roles]\nsparkle = \"base00\"", "unknown theme role"),
            ("[keybindings.normal]\nfly = [\"f\"]", "unknown action"),
            ("[keybindings.normal]\npaste = [\"g\"]", "prefix"),
            ("[general]\ncolour = \"red\"", "unknown field"),
            ("[general\n", "line 1"),
        ] {
            let err = format!("{:#}", parse(text).unwrap_err());
            assert!(err.contains(needle), "{text:?}: {err}");
        }
    }

    #[test]
    fn creates_the_default_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("calendar/config.toml");
        let (_, note) = load_or_create(&path).unwrap();
        assert!(note.unwrap().contains("wrote the default config"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_TOML);
        std::fs::write(&path, "[views]\nstart = \"day\"\n").unwrap();
        let (cfg, note) = load_or_create(&path).unwrap();
        assert!(note.is_none(), "an existing file is never overwritten");
        assert_eq!(cfg.settings.start_view, View::Day);
    }

    #[test]
    fn stamp_follows_symlink_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, link) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("config.toml"),
        );
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "x").unwrap();
        std::os::unix::fs::symlink(&a, &link).unwrap();
        let first = stamp(&link);
        assert!(first.is_some());
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        assert_ne!(
            stamp(&link),
            first,
            "same size and maybe same mtime, different target"
        );
        assert_eq!(stamp(&dir.path().join("missing")), None);
    }
}
