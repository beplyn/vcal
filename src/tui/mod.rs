//! The interactive calendar: terminal setup and the event loop. State and
//! update logic live in `app`, drawing in `ui`.

pub mod app;
pub mod form;
pub mod keymap;
pub mod theme;
pub mod ui;

use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::NaiveDate;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::db::Db;
use crate::settings::{Settings, View};
use crate::time;

pub use app::App;

/// Runs the TUI until the user quits.
pub fn run(db: Db, settings: Settings, date: Option<NaiveDate>, view: Option<View>) -> Result<()> {
    let tz = time::local_tz();
    let mut app = App::new(db, tz, settings, time::now());
    if let Some(v) = view {
        app.view = v;
    }
    if let Some(d) = date {
        app.goto_date(d);
    }
    app.refresh();

    // ratatui::init installs a panic hook that restores the terminal first.
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        let mut last_tick = Instant::now();
        while !app.quit {
            terminal.draw(|f| ui::render(f, &mut app))?;
            let timeout = Duration::from_secs(1).saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key_event(k),
                    _ => {} // resize just redraws
                }
            }
            if last_tick.elapsed() >= Duration::from_secs(1) {
                app.tick(time::now());
                last_tick = Instant::now();
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}
