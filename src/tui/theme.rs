//! The Base16 palette and the semantic roles the UI draws with. Views only
//! call the role methods; which palette slot backs each role is data
//! (`[theme.roles]` in config.toml), so nothing outside this file names a
//! colour or a `baseXX` slot.

use anyhow::{Result, bail};
use ratatui::style::{Color, Modifier, Style};

macro_rules! roles {
    ($($role:ident = $slot:literal: $doc:literal,)*) => {
        /// Palette slot (0x0..=0xF) for each UI role.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Roles { $(#[doc = $doc] pub $role: usize,)* }

        impl Default for Roles {
            fn default() -> Self {
                Roles { $($role: $slot,)* }
            }
        }

        impl Roles {
            pub const NAMES: &[&str] = &[$(stringify!($role),)*];

            /// Points `role` at palette slot `slot`.
            pub fn set(&mut self, role: &str, slot: usize) -> Result<()> {
                match role {
                    $(stringify!($role) => self.$role = slot,)*
                    _ => bail!(
                        "unknown theme role {role:?} (one of: {})",
                        Self::NAMES.join(", ")
                    ),
                }
                Ok(())
            }
        }
    };
}

roles! {
    background = 0x0: "Canvas; also text drawn on coloured blocks.",
    panel = 0x1: "Status bar, dialogs.",
    selection = 0x2: "Cursor cell and visual selection.",
    border = 0x3: "Borders, grid lines, days outside the month.",
    muted = 0x4: "Secondary text, time labels, past events.",
    text = 0x5: "Default foreground.",
    danger = 0x8: "Errors, cancelled events, delete prompts.",
    urgent = 0x9: "Events starting within the hour.",
    today = 0xA: "Today and the current-time line.",
    event = 0xB: "Events, success messages.",
    accent = 0xD: "Selected day header, active tab, focused event.",
    tag = 0xE: "Categories, recurrence marks, holidays.",
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// base00 ..= base0F
    pub base: [Color; 16],
    pub roles: Roles,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// `#rrggbb` or `rrggbb`.
pub fn parse_hex(s: &str) -> Result<Color> {
    let digits = s.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{s:?} isn't a #rrggbb colour");
    }
    Ok(rgb(u32::from_str_radix(digits, 16)?))
}

/// `base0A` (any case) to its slot.
pub fn parse_slot(s: &str) -> Result<usize> {
    let lower = s.trim().to_ascii_lowercase();
    match lower
        .strip_prefix("base")
        .map(|n| usize::from_str_radix(n, 16))
    {
        Some(Ok(n)) if n < 16 && lower.len() == 6 => Ok(n),
        _ => bail!("{s:?} isn't a palette slot (base00 .. base0F)"),
    }
}

impl Default for Theme {
    /// Base16 "Default Dark" (Chris Kempson).
    fn default() -> Self {
        Theme {
            base: [
                rgb(0x181818),
                rgb(0x282828),
                rgb(0x383838),
                rgb(0x585858),
                rgb(0xb8b8b8),
                rgb(0xd8d8d8),
                rgb(0xe8e8e8),
                rgb(0xf8f8f8),
                rgb(0xab4642),
                rgb(0xdc9656),
                rgb(0xf7ca88),
                rgb(0xa1b56c),
                rgb(0x86c1b9),
                rgb(0x7cafc2),
                rgb(0xba8baf),
                rgb(0xa16946),
            ],
            roles: Roles::default(),
        }
    }
}

impl Theme {
    fn c(&self, slot: usize) -> Color {
        self.base[slot]
    }

    /// Canvas background with default text.
    pub fn canvas(&self) -> Style {
        Style::new()
            .bg(self.c(self.roles.background))
            .fg(self.c(self.roles.text))
    }

    pub fn panel(&self) -> Style {
        Style::new()
            .bg(self.c(self.roles.panel))
            .fg(self.c(self.roles.text))
    }

    /// Text on a filled, coloured block (event blocks, mode badges).
    pub fn on_color(&self) -> Color {
        self.c(self.roles.background)
    }

    pub fn selection(&self) -> Color {
        self.c(self.roles.selection)
    }

    pub fn border(&self) -> Style {
        Style::new().fg(self.c(self.roles.border))
    }

    pub fn inactive(&self) -> Style {
        Style::new().fg(self.c(self.roles.border))
    }

    pub fn muted(&self) -> Style {
        Style::new().fg(self.c(self.roles.muted))
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.c(self.roles.text))
    }

    pub fn danger(&self) -> Color {
        self.c(self.roles.danger)
    }

    pub fn urgent(&self) -> Color {
        self.c(self.roles.urgent)
    }

    pub fn today(&self) -> Color {
        self.c(self.roles.today)
    }

    pub fn event(&self) -> Color {
        self.c(self.roles.event)
    }

    pub fn accent(&self) -> Color {
        self.c(self.roles.accent)
    }

    pub fn tag(&self) -> Color {
        self.c(self.roles.tag)
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.accent()).add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours_and_slots() {
        assert_eq!(parse_hex("#7cafc2").unwrap(), Color::Rgb(0x7c, 0xaf, 0xc2));
        assert_eq!(parse_hex("7CAFC2").unwrap(), Color::Rgb(0x7c, 0xaf, 0xc2));
        assert!(parse_hex("#7caf").is_err());
        assert!(parse_hex("#gggggg").is_err());
        assert_eq!(parse_slot("base0A").unwrap(), 10);
        assert_eq!(parse_slot("BASE0f").unwrap(), 15);
        assert!(parse_slot("base10").is_err());
        assert!(parse_slot("base0").is_err());
    }

    #[test]
    fn roles_follow_the_mapping() {
        let mut t = Theme::default();
        assert_eq!(t.accent(), t.base[0xD]);
        t.roles.set("accent", 0xE).unwrap();
        assert_eq!(t.accent(), t.base[0xE]);
        assert_eq!(t.title().fg, Some(t.base[0xE]));
        assert!(t.roles.set("sparkle", 1).is_err());
    }
}
