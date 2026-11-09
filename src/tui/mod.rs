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

use std::path::PathBuf;

use crate::config::Config;
use crate::db::Db;
use crate::settings::View;
use crate::time;

pub use app::App;

/// Command-line choices for how the TUI starts.
#[derive(Debug, Default)]
pub struct Launch {
    pub date: Option<NaiveDate>,
    pub view: Option<View>,
    /// Followed for live reloads.
    pub config_path: Option<PathBuf>,
    /// Shown on the message line at startup.
    pub note: Option<String>,
}

/// Runs the TUI until the user quits.
pub fn run(db: Db, cfg: Config, launch: Launch) -> Result<()> {
    let tz = time::local_tz();
    let database_path = cfg.database_path.clone();
    let mut app = App::new(db, tz, cfg.settings.clone(), time::now());
    app.apply_config(cfg);
    if let Some(path) = launch.config_path {
        app.watch_config(path, database_path);
    }
    if let Some(v) = launch.view {
        app.view = v;
    }
    if let Some(d) = launch.date {
        app.goto_date(d);
    }
    if let Some(note) = launch.note {
        app.info(note);
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
