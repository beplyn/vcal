//! Vim-style key sequences: notation parsing (`gg`, `<C-[>`, `<S-Tab>`), the
//! default bindings per mode, and prefix resolution for multi-key chords.
//! The same notation is what `[keybindings]` in config.toml will use.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key press, normalised so that `G` is `Char('G')` without SHIFT and
/// `<C-[>` is `Esc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    pub const fn plain(code: KeyCode) -> Self {
        Key {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    pub const fn char(c: char) -> Self {
        Key::plain(KeyCode::Char(c))
    }

    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        let code = match code {
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            KeyCode::Char('[') if mods.contains(KeyModifiers::CONTROL) => {
                mods.remove(KeyModifiers::CONTROL);
                KeyCode::Esc
            }
            KeyCode::Char(c) => {
                // Shift is already in the character itself.
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::Char(if mods.contains(KeyModifiers::CONTROL) {
                    c.to_ascii_lowercase()
                } else {
                    c
                })
            }
            other => other,
        };
        Key { code, mods }
    }
}

impl From<KeyEvent> for Key {
    fn from(ev: KeyEvent) -> Self {
        Key::new(ev.code, ev.modifiers)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char('<') => "lt".to_string(),
            KeyCode::Char(c) if self.mods.is_empty() => return write!(f, "{c}"),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Enter => "CR".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::Backspace => "BS".into(),
            KeyCode::Delete => "Del".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        let mut prefix = String::new();
        for (m, p) in [
            (KeyModifiers::CONTROL, "C-"),
            (KeyModifiers::ALT, "A-"),
            (KeyModifiers::SHIFT, "S-"),
        ] {
            if self.mods.contains(m) {
                prefix.push_str(p);
            }
        }
        write!(f, "<{prefix}{name}>")
    }
}

/// Parses a sequence in Vim notation: `gg`, `dd`, `<Esc>`, `<C-[>`, `<S-Tab>`,
/// `<CR>`, `<Space>`, `<lt>`. A `<` without a matching name is literal.
pub fn parse_keys(s: &str) -> Result<Vec<Key>> {
    let mut keys = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(close) = rest.find('>')
            && close > 1
            && let Some(key) = parse_special(&rest[1..close])
        {
            keys.push(key);
            rest = &rest[close + 1..];
            continue;
        }
        keys.push(Key::char(c));
        rest = &rest[c.len_utf8()..];
    }
    if keys.is_empty() {
        bail!("empty key sequence");
    }
    Ok(keys)
}

fn parse_special(name: &str) -> Option<Key> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = name;
    loop {
        let lower = rest.to_ascii_lowercase();
        let m = match lower.get(..2) {
            Some("c-") if rest.len() > 2 => KeyModifiers::CONTROL,
            Some("a-" | "m-") if rest.len() > 2 => KeyModifiers::ALT,
            Some("s-") if rest.len() > 2 => KeyModifiers::SHIFT,
            _ => break,
        };
        mods |= m;
        rest = &rest[2..];
    }
    let code = match rest.to_ascii_lowercase().as_str() {
        "esc" => KeyCode::Esc,
        "cr" | "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "bs" | "backspace" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        "lt" => KeyCode::Char('<'),
        "gt" => KeyCode::Char('>'),
        "bar" => KeyCode::Char('|'),
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        f if f.len() > 1 && f.starts_with('f') => KeyCode::F(f[1..].parse().ok()?),
        _ if rest.chars().count() == 1 && !mods.is_empty() => KeyCode::Char(rest.chars().next()?),
        _ => return None,
    };
    Some(Key::new(code, mods))
}

macro_rules! actions {
    ($($variant:ident = $name:literal: $doc:literal,)*) => {
        /// Everything a key can be bound to.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Action { $($variant,)* }

        impl Action {
            pub const ALL: &[Action] = &[$(Action::$variant,)*];

            /// The name used in config.toml.
            pub fn name(self) -> &'static str {
                match self { $(Action::$variant => $name,)* }
            }

            pub fn describe(self) -> &'static str {
                match self { $(Action::$variant => $doc,)* }
            }
        }

        impl FromStr for Action {
            type Err = anyhow::Error;
            fn from_str(s: &str) -> Result<Self> {
                match s {
                    $($name => Ok(Action::$variant),)*
                    _ => bail!("unknown action {s:?}"),
                }
            }
        }
    };
}

actions! {
    Left = "left": "previous day",
    Down = "down": "next week (month) / next slot / next event",
    Up = "up": "previous week (month) / previous slot / previous event",
    Right = "right": "next day",
    WeekNext = "week_next": "forward one week",
    WeekPrev = "week_prev": "back one week",
    MonthNext = "month_next": "forward one month",
    MonthPrev = "month_prev": "back one month",
    RowStart = "row_start": "start of the week",
    RowEnd = "row_end": "end of the week",
    ViewportFirst = "viewport_first": "first day (or slot) on screen",
    ViewportLast = "viewport_last": "last day (or slot) on screen",
    Today = "today": "jump to today and the current hour",
    ViewNext = "view_next": "next view",
    ViewPrev = "view_prev": "previous view",
    ViewMonth = "view_month": "month view",
    ViewWeek = "view_week": "week view",
    ViewDay = "view_day": "day view",
    ViewAgenda = "view_agenda": "agenda: upcoming events as a list",
    Focus = "focus": "select among the day's events",
    NewEvent = "new_event": "new event here",
    NewAllDay = "new_all_day": "new all-day event",
    Delete = "delete": "delete event(s)",
    Change = "change": "edit event",
    Yank = "yank": "copy event(s)",
    Paste = "paste": "paste copied event(s) here",
    Undo = "undo": "undo the last change",
    Redo = "redo": "redo what was undone",
    ShiftLater = "shift_later": "move selected events later",
    ShiftEarlier = "shift_earlier": "move selected events earlier",
    MoveLater = "move_later": "move the event a slot (or day) later",
    MoveEarlier = "move_earlier": "move the event a slot (or day) earlier",
    Lengthen = "lengthen": "make the event a slot (or day) longer",
    Shorten = "shorten": "make the event a slot (or day) shorter",
    Search = "search": "filter events",
    SearchNext = "search_next": "next match",
    SearchPrev = "search_prev": "previous match",
    CommandLine = "command": "command line",
    Visual = "visual": "select a range",
    Escape = "escape": "cancel / back to normal mode",
    Help = "help": "show key bindings",
    Quit = "quit": "quit",
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyMode {
    Normal,
    Visual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub keys: Vec<Key>,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub normal: Vec<Binding>,
    pub visual: Vec<Binding>,
}

/// Whether a key pressed with no count pending starts one. `0` doesn't: it
/// is `row_start` until a count has begun.
pub fn starts_count(key: Key) -> bool {
    matches!(key.code, KeyCode::Char('1'..='9')) && key.mods.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    Action(Action),
    /// A prefix of some binding; wait for more keys.
    Pending,
    Unbound,
}

const MOVEMENT: &[(&str, Action)] = &[
    ("h", Action::Left),
    ("<Left>", Action::Left),
    ("j", Action::Down),
    ("<Down>", Action::Down),
    ("k", Action::Up),
    ("<Up>", Action::Up),
    ("l", Action::Right),
    ("<Right>", Action::Right),
    ("w", Action::WeekNext),
    ("b", Action::WeekPrev),
    ("}", Action::MonthNext),
    ("{", Action::MonthPrev),
    ("0", Action::RowStart),
    ("<Home>", Action::RowStart),
    ("$", Action::RowEnd),
    ("<End>", Action::RowEnd),
    ("gg", Action::ViewportFirst),
    ("G", Action::ViewportLast),
    ("t", Action::Today),
    (".", Action::Today),
];

const NORMAL: &[(&str, Action)] = &[
    ("<Tab>", Action::ViewNext),
    ("<S-Tab>", Action::ViewPrev),
    ("g1", Action::ViewMonth),
    ("g2", Action::ViewWeek),
    ("g3", Action::ViewDay),
    ("g4", Action::ViewAgenda),
    ("<CR>", Action::Focus),
    ("o", Action::NewEvent),
    ("O", Action::NewAllDay),
    ("dd", Action::Delete),
    ("cc", Action::Change),
    ("yy", Action::Yank),
    ("p", Action::Paste),
    ("u", Action::Undo),
    ("<C-r>", Action::Redo),
    ("J", Action::MoveLater),
    ("K", Action::MoveEarlier),
    (">", Action::Lengthen),
    ("<lt>", Action::Shorten),
    ("/", Action::Search),
    ("n", Action::SearchNext),
    ("N", Action::SearchPrev),
    (":", Action::CommandLine),
    ("v", Action::Visual),
    ("<Esc>", Action::Escape),
    ("<C-c>", Action::Escape),
    ("?", Action::Help),
    ("ZZ", Action::Quit),
];

const VISUAL: &[(&str, Action)] = &[
    ("d", Action::Delete),
    ("x", Action::Delete),
    ("y", Action::Yank),
    (">", Action::ShiftLater),
    ("<lt>", Action::ShiftEarlier),
    ("o", Action::NewEvent),
    ("<Esc>", Action::Escape),
    ("<C-c>", Action::Escape),
    ("v", Action::Escape),
];

fn table(entries: &[&[(&str, Action)]]) -> Vec<Binding> {
    entries
        .iter()
        .flat_map(|t| t.iter())
        .map(|(keys, action)| Binding {
            keys: parse_keys(keys).expect("default binding parses"),
            action: *action,
        })
        .collect()
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap {
            normal: table(&[MOVEMENT, NORMAL]),
            visual: table(&[MOVEMENT, VISUAL]),
        }
    }
}

impl Keymap {
    fn bindings(&self, mode: KeyMode) -> &[Binding] {
        match mode {
            KeyMode::Normal => &self.normal,
            KeyMode::Visual => &self.visual,
        }
    }

    pub fn resolve(&self, mode: KeyMode, pending: &[Key]) -> Resolved {
        let mut prefix = false;
        for b in self.bindings(mode) {
            if b.keys == pending {
                return Resolved::Action(b.action);
            }
            prefix |= b.keys.starts_with(pending);
        }
        if prefix {
            Resolved::Pending
        } else {
            Resolved::Unbound
        }
    }

    /// The bindings for an action, in notation, for the help screen.
    pub fn keys_for(&self, mode: KeyMode, action: Action) -> Vec<String> {
        self.bindings(mode)
            .iter()
            .filter(|b| b.action == action)
            .map(|b| b.keys.iter().map(Key::to_string).collect())
            .collect()
    }

    /// Replaces every binding of `action` in `mode` with `sequences` (empty
    /// unbinds it). A sequence another action had moves to this one.
    pub fn rebind(&mut self, mode: KeyMode, action: Action, sequences: Vec<Vec<Key>>) {
        let list = match mode {
            KeyMode::Normal => &mut self.normal,
            KeyMode::Visual => &mut self.visual,
        };
        list.retain(|b| b.action != action && !sequences.contains(&b.keys));
        list.extend(sequences.into_iter().map(|keys| Binding { keys, action }));
    }

    /// Bindings that could never fire: a strict prefix of another in the same
    /// mode (the longer one would need a timeout), or one starting with
    /// `1`-`9`, which begins a count.
    pub fn conflicts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let show = |b: &Binding| b.keys.iter().map(Key::to_string).collect::<String>();
        for (mode, list) in [("normal", &self.normal), ("visual", &self.visual)] {
            for b in list.iter().filter(|b| starts_count(b.keys[0])) {
                out.push(format!(
                    "{mode}: \"{}\" ({}) starts with a count digit",
                    show(b),
                    b.action.name()
                ));
            }
            for short in list {
                for long in list {
                    if short.keys.len() < long.keys.len() && long.keys.starts_with(&short.keys) {
                        out.push(format!(
                            "{mode}: \"{}\" ({}) hides \"{}\" ({})",
                            show(short),
                            short.action.name(),
                            show(long),
                            long.action.name()
                        ));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation() {
        assert_eq!(parse_keys("gg").unwrap(), [Key::char('g'), Key::char('g')]);
        assert_eq!(parse_keys("<C-[>").unwrap(), [Key::plain(KeyCode::Esc)]);
        assert_eq!(
            parse_keys("<S-Tab>").unwrap(),
            [Key::new(KeyCode::BackTab, KeyModifiers::NONE)]
        );
        assert_eq!(parse_keys("<").unwrap(), [Key::char('<')]);
        assert_eq!(parse_keys("<lt>").unwrap(), [Key::char('<')]);
        assert_eq!(parse_keys("<nope>").unwrap().len(), 6);
        assert_eq!(
            parse_keys("<C-d>").unwrap(),
            [Key::new(KeyCode::Char('d'), KeyModifiers::CONTROL)]
        );
        assert!(parse_keys("").is_err());
    }

    #[test]
    fn display_round_trips() {
        for s in ["gg", "<Esc>", "<S-Tab>", "<C-d>", "<lt>", "<Space>", "G"] {
            let keys = parse_keys(s).unwrap();
            let shown: String = keys.iter().map(Key::to_string).collect();
            assert_eq!(parse_keys(&shown).unwrap(), keys, "{s}");
        }
    }

    #[test]
    fn terminal_events_normalise() {
        // Terminals report G as Char('G') + SHIFT.
        assert_eq!(
            Key::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
            Key::char('G')
        );
        assert_eq!(
            Key::new(KeyCode::Char('['), KeyModifiers::CONTROL),
            Key::plain(KeyCode::Esc)
        );
    }

    #[test]
    fn chords_resolve() {
        let km = Keymap::default();
        let g = Key::char('g');
        let d = Key::char('d');
        assert_eq!(km.resolve(KeyMode::Normal, &[g]), Resolved::Pending);
        assert_eq!(
            km.resolve(KeyMode::Normal, &[g, g]),
            Resolved::Action(Action::ViewportFirst)
        );
        assert_eq!(km.resolve(KeyMode::Normal, &[d]), Resolved::Pending);
        assert_eq!(
            km.resolve(KeyMode::Normal, &[d, d]),
            Resolved::Action(Action::Delete)
        );
        assert_eq!(
            km.resolve(KeyMode::Visual, &[d]),
            Resolved::Action(Action::Delete)
        );
        assert_eq!(km.resolve(KeyMode::Normal, &[d, g]), Resolved::Unbound);
    }

    #[test]
    fn no_binding_shadows_another() {
        // A complete binding that is also a prefix would need a timeout to resolve.
        assert_eq!(Keymap::default().conflicts(), Vec::<String>::new());
    }

    #[test]
    fn rebinding() {
        let mut km = Keymap::default();
        let keys = |s: &str| parse_keys(s).unwrap();
        km.rebind(KeyMode::Normal, Action::Quit, vec![keys("q"), keys("t")]);
        assert_eq!(
            km.resolve(KeyMode::Normal, &keys("q")),
            Resolved::Action(Action::Quit)
        );
        assert_eq!(
            km.resolve(KeyMode::Normal, &keys("ZZ")),
            Resolved::Unbound,
            "old keys go"
        );
        // "t" moved from today to quit; today keeps "."
        assert_eq!(
            km.resolve(KeyMode::Normal, &keys("t")),
            Resolved::Action(Action::Quit)
        );
        assert_eq!(km.keys_for(KeyMode::Normal, Action::Today), ["."]);
        assert_eq!(
            km.resolve(KeyMode::Visual, &keys("t")),
            Resolved::Action(Action::Today)
        );
        km.rebind(KeyMode::Normal, Action::Help, vec![]);
        assert!(km.keys_for(KeyMode::Normal, Action::Help).is_empty());
        km.rebind(KeyMode::Normal, Action::Paste, vec![keys("g")]);
        assert_eq!(
            km.conflicts()[0],
            "normal: \"g\" (paste) hides \"gg\" (viewport_first)"
        );
    }

    #[test]
    fn digits_are_counts() {
        let mut km = Keymap::default();
        km.rebind(
            KeyMode::Visual,
            Action::ViewMonth,
            vec![parse_keys("1").unwrap()],
        );
        assert_eq!(
            km.conflicts(),
            ["visual: \"1\" (view_month) starts with a count digit"]
        );
        // 0 is row_start until a count has begun
        assert!(!starts_count(Key::char('0')));
    }

    #[test]
    fn action_names_round_trip() {
        for a in Action::ALL {
            assert_eq!(a.name().parse::<Action>().unwrap(), *a);
        }
    }
}
