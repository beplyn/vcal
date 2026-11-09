//! The Base16 palette and the roles the UI draws with. Views only use the
//! role methods; colours never appear as literals anywhere else.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// base00 ..= base0F
    pub base: [Color; 16],
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
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
        }
    }
}

impl Theme {
    pub fn base(&self, i: usize) -> Color {
        self.base[i]
    }

    /// base00 canvas with base05 text.
    pub fn canvas(&self) -> Style {
        Style::new().bg(self.base[0x0]).fg(self.base[0x5])
    }

    /// base01: panels, dialogs, status bar.
    pub fn panel(&self) -> Style {
        Style::new().bg(self.base[0x1]).fg(self.base[0x5])
    }

    /// base02: cursor cell and visual selection.
    pub fn selection(&self) -> Color {
        self.base[0x2]
    }

    /// base03: borders, grid lines, days outside the month.
    pub fn border(&self) -> Style {
        Style::new().fg(self.base[0x3])
    }

    pub fn inactive(&self) -> Style {
        Style::new().fg(self.base[0x3])
    }

    /// base04: secondary text, time labels, past events.
    pub fn muted(&self) -> Style {
        Style::new().fg(self.base[0x4])
    }

    pub fn text(&self) -> Style {
        Style::new().fg(self.base[0x5])
    }

    /// base08: errors, cancellations, delete prompts.
    pub fn danger(&self) -> Color {
        self.base[0x8]
    }

    /// base09: events starting soon.
    pub fn urgent(&self) -> Color {
        self.base[0x9]
    }

    /// base0A: today and the current-time line.
    pub fn today(&self) -> Color {
        self.base[0xA]
    }

    /// base0B: confirmed events, success messages.
    pub fn event(&self) -> Color {
        self.base[0xB]
    }

    /// base0D: selected day header, active tab, focused event.
    pub fn accent(&self) -> Color {
        self.base[0xD]
    }

    /// base0E: categories, tags, recurrence marks.
    pub fn tag(&self) -> Color {
        self.base[0xE]
    }

    pub fn title(&self) -> Style {
        Style::new().fg(self.accent()).add_modifier(Modifier::BOLD)
    }
}
