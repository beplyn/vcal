//! Rendering. Pure function of `App` (apart from recording how many slot
//! rows fit, so scrolling can follow the cursor).

use chrono::{Datelike, Duration, NaiveDate};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget, Wrap};

use super::app::{App, Focus, Mode, Prompt};
use super::form::Field;
use super::keymap::{Action, KeyMode};
use crate::model::{Occurrence, Status};
use crate::settings::View;
use crate::time;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.buffer_mut().set_style(area, app.theme.canvas());
    if area.width < 20 || area.height < 6 {
        put(
            f.buffer_mut(),
            area.x,
            area.y,
            area.width,
            "terminal too small",
            app.theme.muted(),
        );
        return;
    }
    let [header, body, status, line] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(f.buffer_mut(), header, app);
    match app.view {
        View::Month => render_month(f, body, app),
        View::Week => {
            let first = app.week_start(app.cursor);
            let days: Vec<_> = (0..7).map(|i| first + Duration::days(i)).collect();
            render_timeline(f.buffer_mut(), body, app, &days);
        }
        View::Day => {
            let w = (body.width * 2 / 5).clamp(24.min(body.width), 60);
            let [timeline, agenda] =
                Layout::horizontal([Constraint::Length(w), Constraint::Min(0)]).areas(body);
            render_timeline(f.buffer_mut(), timeline, app, &[app.cursor]);
            render_agenda(f, agenda, app, true);
        }
    }
    render_status(f.buffer_mut(), status, app);
    render_line(f, line, app);

    match app.mode {
        Mode::Insert => render_form(f, area, app),
        Mode::Prompt => render_prompt(f, area, app),
        Mode::Help => render_help(f, area, app),
        _ => {}
    }
}

/// Text clipped to `width` cells.
fn put(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    let area = buf.area;
    if width > 0 && y >= area.y && y < area.bottom() && x >= area.x && x < area.right() {
        let width = width.min(area.right() - x);
        buf.set_stringn(x, y, text, width as usize, style);
    }
}

fn occ_style(app: &App, o: &Occurrence) -> Style {
    let t = &app.theme;
    if o.status == Status::Cancelled {
        Style::new()
            .fg(t.danger())
            .add_modifier(Modifier::CROSSED_OUT)
    } else if app.is_past(o) {
        t.muted()
    } else if app.is_soon(o) {
        Style::new().fg(t.urgent())
    } else if o.status == Status::Tentative {
        Style::new().fg(t.event()).add_modifier(Modifier::ITALIC)
    } else {
        Style::new().fg(t.event())
    }
}

fn time_range(app: &App, o: &Occurrence) -> String {
    if o.all_day {
        return "all day".into();
    }
    let s = &app.settings;
    format!(
        "{}–{}",
        s.time(time::in_tz(o.start, app.tz)),
        s.time(time::in_tz(o.end, app.tz))
    )
}

fn is_holiday(o: &Occurrence) -> bool {
    o.category
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case("holiday"))
}

// --- chrome --------------------------------------------------------------

fn render_header(buf: &mut Buffer, area: Rect, app: &App) {
    let t = &app.theme;
    let mut x = area.x;
    put(buf, x, area.y, area.width, " vcal ", t.title());
    x += 7;
    for v in View::ALL {
        let label = format!(" {} ", v.name());
        let style = if v == app.view {
            Style::new()
                .bg(t.accent())
                .fg(t.base(0))
                .add_modifier(Modifier::BOLD)
        } else {
            t.muted()
        };
        put(
            buf,
            x,
            area.y,
            area.right().saturating_sub(x),
            &label,
            style,
        );
        x += label.len() as u16 + 1;
    }
    let c = app.cursor;
    let title = match app.view {
        View::Month => c.format("%B %Y").to_string(),
        View::Week => {
            let first = app.week_start(c);
            let last = first + Duration::days(6);
            format!(
                "Week {} · {} – {}",
                c.iso_week().week(),
                first.format("%-d %b"),
                last.format("%-d %b %Y")
            )
        }
        View::Day => c.format("%A %-d %B %Y").to_string(),
    };
    let w = title.chars().count() as u16 + 1;
    if area.width > x - area.x + w {
        put(
            buf,
            area.right() - w,
            area.y,
            w,
            &title,
            t.text().add_modifier(Modifier::BOLD),
        );
    }
}

fn render_status(buf: &mut Buffer, area: Rect, app: &App) {
    let t = &app.theme;
    buf.set_style(area, t.panel());
    let mode_bg = match app.mode {
        Mode::Normal | Mode::Help => t.accent(),
        Mode::Visual => t.tag(),
        Mode::Insert => t.event(),
        Mode::Command | Mode::Search => t.today(),
        Mode::Prompt => t.danger(),
    };
    let label = format!(" {} ", app.mode.label());
    let mut spans = vec![
        Span::styled(
            label,
            Style::new()
                .bg(mode_bg)
                .fg(t.base(0))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];
    if app.focus == Focus::Events {
        spans.push(Span::styled("events ", t.muted()));
    }
    if let Some(f) = &app.filter {
        spans.push(Span::styled(format!("/{f} "), Style::new().fg(t.tag())));
    }
    if !app.pending.is_empty() {
        let keys: String = app.pending.iter().map(|k| k.to_string()).collect();
        spans.push(Span::styled(keys, t.text().add_modifier(Modifier::BOLD)));
    }
    Paragraph::new(Line::from(spans)).render(area, buf);

    let n = app.day_events(app.cursor).len();
    let right = format!(
        "{} event{} · {} {} ",
        n,
        if n == 1 { "" } else { "s" },
        time::local_date(app.now, app.tz).format("%a %-d %b"),
        app.settings.time(time::in_tz(app.now, app.tz)),
    );
    let w = right.chars().count() as u16;
    if area.width > w + 20 {
        put(buf, area.right() - w, area.y, w, &right, t.panel());
    }
}

fn render_line(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let buf = f.buffer_mut();
    match app.mode {
        Mode::Command | Mode::Search => {
            let prefix = if app.mode == Mode::Command { ":" } else { "/" };
            put(buf, area.x, area.y, area.width, prefix, t.text());
            put(
                buf,
                area.x + 1,
                area.y,
                area.width.saturating_sub(1),
                &app.line.text,
                t.text(),
            );
            let col: usize = app
                .line
                .text
                .chars()
                .take(app.line.cursor)
                .map(char_width)
                .sum();
            f.set_cursor_position(Position::new(area.x + 1 + col as u16, area.y));
        }
        _ => {
            let (text, style) = match &app.message {
                Some(m) if m.error => (m.text.clone(), Style::new().fg(t.danger())),
                Some(m) => (m.text.clone(), Style::new().fg(t.event())),
                None => (
                    "? help · : command · o new · v select · / search".into(),
                    t.muted(),
                ),
            };
            put(buf, area.x, area.y, area.width, &text, style);
        }
    }
}

fn char_width(c: char) -> usize {
    Span::raw(c.to_string()).width()
}

// --- month ---------------------------------------------------------------

fn render_month(f: &mut Frame, area: Rect, app: &mut App) {
    let (grid, agenda) = if area.width >= 100 {
        let [g, a] = Layout::horizontal([Constraint::Min(0), Constraint::Length(38)]).areas(area);
        (g, Some(a))
    } else if area.height >= 30 {
        let [g, a] = Layout::vertical([Constraint::Min(0), Constraint::Length(9)]).areas(area);
        (g, Some(a))
    } else {
        (area, None)
    };
    render_month_grid(f.buffer_mut(), grid, app);
    if let Some(a) = agenda {
        render_agenda(f, a, app, false);
    }
}

fn render_month_grid(buf: &mut Buffer, area: Rect, app: &App) {
    let t = &app.theme;
    if area.width < 14 || area.height < 4 {
        return;
    }
    let month_first = app.cursor.with_day(1).unwrap_or(app.cursor);
    let grid_first = app.week_start(month_first);
    let next_month = month_first + chrono::Months::new(1);
    let rows = ((next_month - grid_first).num_days() as u16).div_ceil(7);

    let col_x = |c: u16| area.x + c * area.width / 7;
    let cw = area.width / 7;
    let ch = (area.height - 1) / rows;
    // weekday header
    for col in 0..7u16 {
        let day = grid_first + Duration::days(i64::from(col));
        let x = col_x(col);
        let style = if day.weekday() == app.cursor.weekday() {
            t.title()
        } else {
            t.muted()
        };
        put(
            buf,
            x + 1,
            area.y,
            cw.saturating_sub(1),
            &day.format("%a").to_string(),
            style,
        );
    }
    for row in 0..rows {
        for col in 0..7u16 {
            let date = grid_first + Duration::days(i64::from(row * 7 + col));
            let width = col_x(col + 1) - col_x(col);
            let height = if row == rows - 1 {
                area.height - 1 - row * ch
            } else {
                ch
            };
            let rect = Rect::new(col_x(col), area.y + 1 + row * ch, width, height);
            render_day_cell(
                buf,
                rect,
                app,
                date,
                col > 0,
                row == 0,
                date.month() == app.cursor.month(),
            );
        }
    }
}

fn render_day_cell(
    buf: &mut Buffer,
    rect: Rect,
    app: &App,
    date: NaiveDate,
    left: bool,
    top: bool,
    in_month: bool,
) {
    let t = &app.theme;
    let is_cursor = date == app.cursor;
    let selected = app.in_selection(super::app::Point { date, slot: 0 });
    // Grid lines drawn by hand so cells meet in proper junctions.
    let rule = if is_cursor {
        Style::new().fg(t.accent())
    } else {
        t.border()
    };
    put(
        buf,
        rect.x,
        rect.y,
        rect.width,
        &"─".repeat(rect.width as usize),
        rule,
    );
    let off = u16::from(left);
    if left {
        put(
            buf,
            rect.x,
            rect.y,
            1,
            if top { "┬" } else { "┼" },
            t.border(),
        );
        for y in rect.y + 1..rect.bottom() {
            put(buf, rect.x, y, 1, "│", t.border());
        }
    }
    let inner = Rect::new(
        rect.x + off,
        rect.y + 1,
        rect.width.saturating_sub(off),
        rect.height.saturating_sub(1),
    );
    if is_cursor || selected {
        buf.set_style(inner, Style::new().bg(t.selection()));
    }
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let events = app.day_events(date);
    let number = format!("{:>2}", date.day());
    let number_style = if date == app.today() {
        Style::new()
            .bg(t.today())
            .fg(t.base(0))
            .add_modifier(Modifier::BOLD)
    } else if !in_month {
        t.inactive()
    } else if is_cursor {
        t.text().add_modifier(Modifier::BOLD)
    } else {
        t.text()
    };
    put(buf, inner.x, inner.y, inner.width, &number, number_style);
    let mut x = inner.x + 3;
    if events.iter().any(|o| is_holiday(o)) {
        put(
            buf,
            x,
            inner.y,
            inner.right().saturating_sub(x),
            "★",
            Style::new().fg(t.tag()),
        );
        x += 2;
    }
    if inner.height == 1 {
        // no room for titles: one dot per event
        let dots = "•".repeat(events.len().min(inner.right().saturating_sub(x) as usize));
        put(
            buf,
            x,
            inner.y,
            inner.right().saturating_sub(x),
            &dots,
            Style::new().fg(t.event()),
        );
        return;
    }

    let room = (inner.height - 1) as usize;
    let shown = if events.len() > room {
        room - 1
    } else {
        events.len()
    };
    for (i, o) in events.iter().take(shown).enumerate() {
        let y = inner.y + 1 + i as u16;
        let text = if o.all_day {
            format!("▌{}", o.title)
        } else {
            format!(
                "{} {}",
                app.settings.time(time::in_tz(o.start, app.tz)),
                o.title
            )
        };
        let mut style = occ_style(app, o);
        if is_cursor && app.focus == Focus::Events && app.is_target(o) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        put(buf, inner.x, y, inner.width, &text, style);
    }
    if shown < events.len() {
        let more = format!("+{} more", events.len() - shown);
        put(
            buf,
            inner.x,
            inner.y + 1 + shown as u16,
            inner.width,
            &more,
            t.muted(),
        );
    }
}

// --- agenda --------------------------------------------------------------

fn duration_label(secs: i64) -> String {
    let m = secs / 60;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m:02}"),
    }
}

/// One cell per 15 minutes, capped, with a half block for the remainder.
fn duration_bar(secs: i64) -> String {
    let quarters = (secs / 900).clamp(0, 24);
    let mut bar = "█".repeat((quarters / 2) as usize);
    if quarters % 2 == 1 {
        bar.push('▌');
    }
    if bar.is_empty() {
        bar.push('▏');
    }
    bar
}

fn render_agenda(f: &mut Frame, area: Rect, app: &App, detailed: bool) {
    let t = &app.theme;
    let date = app.cursor;
    let mut title = vec![Span::styled(
        date.format(" %a %-d %B ").to_string(),
        t.title(),
    )];
    if date == app.today() {
        title.push(Span::styled("today ", Style::new().fg(t.today())));
    }
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(if app.focus == Focus::Events {
            Style::new().fg(t.accent())
        } else {
            t.border()
        })
        .title(Line::from(title));
    let inner = block.inner(area);
    block.render(area, f.buffer_mut());

    let events = app.day_events(date);
    if events.is_empty() {
        let hint = if app.filter.is_some() {
            "No matches"
        } else {
            "No events · o to add"
        };
        put(
            f.buffer_mut(),
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(1),
            hint,
            t.muted(),
        );
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_at = 0;
    let target = app.target();
    for o in &events {
        let is_target = target.as_ref().is_some_and(|x| x == *o);
        if is_target {
            selected_at = lines.len();
        }
        let marker = if is_target && app.focus == Focus::Events {
            "▶ "
        } else {
            "  "
        };
        let mut head = vec![
            Span::styled(marker, Style::new().fg(t.accent())),
            Span::styled(format!("{:<11} ", time_range(app, o)), t.muted()),
            Span::styled(
                o.title.clone(),
                occ_style(app, o).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(c) = &o.category {
            head.push(Span::styled(format!(" [{c}]"), Style::new().fg(t.tag())));
        }
        if o.recurring() {
            head.push(Span::styled(" ↻", Style::new().fg(t.tag())));
        }
        let mut entry = vec![Line::from(head)];
        if detailed {
            let mut sub = vec![Span::raw("    ")];
            if !o.all_day {
                sub.push(Span::styled(
                    duration_bar(o.end - o.start),
                    occ_style(app, o),
                ));
                sub.push(Span::styled(
                    format!(" {}", duration_label(o.end - o.start)),
                    t.muted(),
                ));
            }
            if let Some(l) = &o.location {
                sub.push(Span::styled(format!("  @ {l}"), t.muted()));
            }
            if sub.len() > 1 {
                entry.push(Line::from(sub));
            }
            for d in o
                .description
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(if is_target { 6 } else { 2 })
            {
                entry.push(Line::from(Span::styled(format!("    {d}"), t.muted())));
            }
        }
        if is_target {
            for l in &mut entry {
                l.style = Style::new().bg(t.selection());
            }
        }
        lines.extend(entry);
    }
    let scroll = (selected_at as u16).saturating_sub(inner.height.saturating_sub(4));
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .render(inner, f.buffer_mut());
}

// --- week / day timeline -------------------------------------------------

fn render_timeline(buf: &mut Buffer, area: Rect, app: &mut App, days: &[NaiveDate]) {
    let gutter: u16 = if app.settings.clock_24h { 6 } else { 5 };
    if area.width <= gutter + days.len() as u16 || area.height < 4 {
        return;
    }
    let cols = days.len() as u16;
    let span = area.width - gutter;
    let col_x = |i: u16| area.x + gutter + i * span / cols;
    let col_w = |i: u16| col_x(i + 1) - col_x(i);

    // day headers
    for (i, d) in days.iter().enumerate() {
        let i = i as u16;
        let style = if *d == app.cursor {
            app.theme.title()
        } else if *d == app.today() {
            Style::new()
                .fg(app.theme.today())
                .add_modifier(Modifier::BOLD)
        } else {
            app.theme.muted()
        };
        put(
            buf,
            col_x(i) + 1,
            area.y,
            col_w(i).saturating_sub(1),
            &d.format("%a %-d").to_string(),
            style,
        );
    }

    // all-day lane
    let all_day: Vec<Vec<Occurrence>> = days
        .iter()
        .map(|d| {
            app.day_events(*d)
                .into_iter()
                .filter(|o| o.all_day)
                .cloned()
                .collect()
        })
        .collect();
    let lanes = all_day.iter().map(Vec::len).max().unwrap_or(0).min(3) as u16;
    for (i, list) in all_day.iter().enumerate() {
        let i = i as u16;
        for (j, o) in list.iter().take(lanes as usize).enumerate() {
            let mut style = occ_style(app, o);
            if app.is_target(o) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let text = if j as u16 == lanes - 1 && list.len() > lanes as usize {
                format!("+{} more", list.len() - j)
            } else {
                format!("▌{}", o.title)
            };
            put(
                buf,
                col_x(i) + 1,
                area.y + 1 + j as u16,
                col_w(i).saturating_sub(1),
                &text,
                style,
            );
        }
    }
    let rule_y = area.y + 1 + lanes;
    put(
        buf,
        area.x,
        rule_y,
        area.width,
        &"─".repeat(area.width as usize),
        app.theme.border(),
    );

    let top = rule_y + 1;
    let rows = area.bottom().saturating_sub(top) as u32;
    app.viewport_slots = rows.max(1);
    app.keep_slot_visible();
    let app = &*app;
    let t = &app.theme;
    let s = &app.settings;
    let end_slot = s.end_slot();
    let now_slot = app.slot_of(app.now);
    let today = app.today();

    for r in 0..rows {
        let slot = app.scroll + r;
        if slot >= end_slot {
            break;
        }
        let y = top + r as u16;
        let minutes = slot * s.slot_minutes;
        let is_now_row = days.contains(&today) && slot == now_slot;
        if minutes.is_multiple_of(60) || is_now_row {
            let label = if minutes.is_multiple_of(60) {
                s.hour_label(minutes / 60)
            } else {
                String::new()
            };
            let style = if is_now_row {
                Style::new().fg(t.today()).add_modifier(Modifier::BOLD)
            } else {
                t.muted()
            };
            put(
                buf,
                area.x,
                y,
                gutter,
                &format!("{label:<w$}", w = gutter as usize - 1),
                style,
            );
            if is_now_row {
                put(buf, area.x + gutter - 1, y, 1, "▶", style);
            }
        }
        for (i, d) in days.iter().enumerate() {
            let i = i as u16;
            let x = col_x(i);
            put(buf, x, y, 1, "│", t.border());
            let cell = Rect::new(x + 1, y, col_w(i).saturating_sub(1), 1);
            let here = super::app::Point { date: *d, slot };
            let on_cursor = *d == app.cursor && slot == app.slot && app.focus == Focus::Grid;
            if on_cursor || app.in_selection(here) {
                buf.set_style(cell, Style::new().bg(t.selection()));
            }
            if *d == today && slot == now_slot {
                put(
                    buf,
                    cell.x,
                    y,
                    cell.width,
                    &"┄".repeat(cell.width as usize),
                    Style::new().fg(t.today()),
                );
            }
        }
    }

    // timed events, side by side when they overlap
    for (i, d) in days.iter().enumerate() {
        let i = i as u16;
        let events: Vec<(&Occurrence, u32, u32)> = app
            .day_events(*d)
            .into_iter()
            .filter_map(|o| app.slot_span(o, *d).map(|(a, b)| (o, a, b)))
            .collect();
        let layout = overlap_lanes(&events.iter().map(|(_, a, b)| (*a, *b)).collect::<Vec<_>>());
        let inner_w = col_w(i).saturating_sub(1);
        for ((o, a, b), (lane, n)) in events.into_iter().zip(layout) {
            let n = n as u16;
            let lw = (inner_w / n).max(1);
            let first = a.max(app.scroll);
            let last = b.min(app.scroll + rows);
            if first >= last {
                continue;
            }
            let lane = lane as u16;
            let x = col_x(i) + 1 + lane * lw;
            let w = if lane == n - 1 {
                inner_w - lane * lw
            } else {
                lw.saturating_sub(1).max(1)
            };
            let bg = if app.is_target(o) {
                t.accent()
            } else {
                occ_style(app, o).fg.unwrap_or(t.event())
            };
            let style = Style::new().bg(bg).fg(t.base(0));
            for slot in first..last {
                let y = top + (slot - app.scroll) as u16;
                buf.set_style(Rect::new(x, y, w, 1), style);
                put(buf, x, y, w, &" ".repeat(w as usize), style);
            }
            let y0 = top + (first - app.scroll) as u16;
            let lead = if a < app.scroll { "↑" } else { "" };
            let clock = s.time(time::in_tz(o.start, app.tz));
            // narrow lanes: the title says more than the time
            let head = if (w as usize) < clock.len() + 6 {
                format!("{lead}{}", o.title)
            } else {
                format!("{lead}{clock} {}", o.title)
            };
            put(buf, x, y0, w, &head, style.add_modifier(Modifier::BOLD));
            if last - first > 1
                && let Some(c) = o.category.as_deref().or(o.location.as_deref())
            {
                put(buf, x, y0 + 1, w, c, style);
            }
        }
    }
}

// --- popups --------------------------------------------------------------

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn popup(f: &mut Frame, rect: Rect, app: &App, title: &str, border: Style) -> Rect {
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Span::styled(title, app.theme.title()))
        .style(app.theme.panel());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

fn render_form(f: &mut Frame, area: Rect, app: &App) {
    let Some(form) = &app.form else { return };
    let t = &app.theme;
    let fields = form.fields();
    let rect = centered(area, 66, fields.len() as u16 + 6);
    let inner = popup(f, rect, app, form.heading(), Style::new().fg(t.accent()));
    let label_w = 10;
    let mut cursor = None;
    for (i, field) in fields.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let focused = *field == form.focus;
        let label_style = if focused { t.title() } else { t.muted() };
        let buf = f.buffer_mut();
        put(buf, inner.x + 1, y, label_w, field.label(), label_style);
        let input_area = Rect::new(
            inner.x + 1 + label_w,
            y,
            inner.width.saturating_sub(label_w + 2),
            1,
        );
        if focused {
            buf.set_style(input_area, Style::new().bg(t.selection()));
        }
        match form.input(*field) {
            None => {
                let mark = if form.all_day { "[x]" } else { "[ ]" };
                put(buf, input_area.x, y, input_area.width, mark, t.text());
            }
            Some(input) => {
                // keep the cursor in view on long lines
                let col: usize = input.text.chars().take(input.cursor).map(char_width).sum();
                let skip = col.saturating_sub(input_area.width.saturating_sub(1) as usize);
                let shown: String = input.text.chars().skip(skip).collect();
                put(buf, input_area.x, y, input_area.width, &shown, t.text());
                if focused {
                    cursor = Some(Position::new(input_area.x + (col - skip) as u16, y));
                }
            }
        }
    }
    let buf = f.buffer_mut();
    let hint_y = inner.y + fields.len() as u16 + 1;
    if let Some(err) = &form.error {
        put(
            buf,
            inner.x + 1,
            hint_y,
            inner.width.saturating_sub(2),
            err,
            Style::new().fg(t.danger()),
        );
    } else {
        put(
            buf,
            inner.x + 1,
            hint_y,
            inner.width.saturating_sub(2),
            form.hint(form.focus),
            t.muted(),
        );
    }
    let footer = if form.focus == Field::AllDay {
        "Space toggle · Enter save · Esc cancel · Tab next"
    } else {
        "Enter save · Esc cancel · Tab/↓ next · S-Tab/↑ previous"
    };
    put(
        buf,
        inner.x + 1,
        hint_y + 1,
        inner.width.saturating_sub(2),
        footer,
        t.muted(),
    );
    if let Some(p) = cursor {
        f.set_cursor_position(p);
    }
}

fn render_prompt(f: &mut Frame, area: Rect, app: &App) {
    let Some(prompt) = &app.prompt else { return };
    let t = &app.theme;
    let (title, lines, border) = match prompt {
        Prompt::Delete(list) => {
            let what = match list.as_slice() {
                [one] if one.original_start.is_some() => {
                    format!("Delete this occurrence of \"{}\"?", one.title)
                }
                [one]
                    if one.recurring()
                        || app
                            .db
                            .get(&one.event_id)
                            .ok()
                            .flatten()
                            .is_some_and(|e| e.is_recurring()) =>
                {
                    format!("Delete every occurrence of \"{}\"?", one.title)
                }
                [one] => format!("Delete \"{}\"?", one.title),
                many => format!("Delete {} events?", many.len()),
            };
            (
                " Delete ",
                vec![
                    what,
                    String::new(),
                    "y / Enter  delete     n / Esc  keep".into(),
                ],
                t.danger(),
            )
        }
        Prompt::Scope { occ, then } => {
            let verb = match then {
                super::app::ScopeAction::Delete => "Delete",
                super::app::ScopeAction::Change => "Edit",
            };
            (
                " Repeating event ",
                vec![
                    format!("\"{}\" repeats.", occ.title),
                    String::new(),
                    format!("o  {verb} this occurrence"),
                    format!("a  {verb} all occurrences"),
                    "Esc  cancel".into(),
                ],
                t.tag(),
            )
        }
    };
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(20) as u16 + 6;
    let rect = centered(area, width.max(30), lines.len() as u16 + 2);
    let inner = popup(f, rect, app, title, Style::new().fg(border));
    let text: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, t.text())))
        .collect();
    Paragraph::new(text).render(
        Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(1),
            ..inner
        },
        f.buffer_mut(),
    );
}

fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.theme;
    let mut entries: Vec<(String, &str)> = Vec::new();
    for a in Action::ALL {
        let keys = app.keymap.keys_for(KeyMode::Normal, *a);
        if !keys.is_empty() {
            entries.push((keys.join(" "), a.describe()));
        }
    }
    for a in [
        Action::Delete,
        Action::Yank,
        Action::ShiftLater,
        Action::ShiftEarlier,
        Action::NewEvent,
    ] {
        let keys = app.keymap.keys_for(KeyMode::Visual, a);
        let desc = match a {
            Action::Delete => "visual: delete selected",
            Action::Yank => "visual: copy selected",
            Action::NewEvent => "visual: new event over selection",
            other => other.describe(),
        };
        entries.push((keys.join(" "), desc));
    }
    entries.push((
        ":q :jump :view :import".into(),
        "commands (also :today :noh :reload)",
    ));

    let col_w = 46u16;
    let cols = if area.width >= col_w * 2 + 4 { 2 } else { 1 };
    let per_col = entries.len().div_ceil(cols);
    let rect = centered(area, col_w * cols as u16 + 4, per_col as u16 + 4);
    let inner = popup(
        f,
        rect,
        app,
        " Keys · any key to close ",
        Style::new().fg(t.accent()),
    );
    let buf = f.buffer_mut();
    for (i, (keys, desc)) in entries.iter().enumerate() {
        let (c, r) = (i / per_col, i % per_col);
        let x = inner.x + 1 + c as u16 * (col_w + 1);
        let y = inner.y + 1 + r as u16;
        if y >= inner.bottom() {
            continue;
        }
        put(buf, x, y, 14, keys, t.title());
        put(buf, x + 15, y, col_w - 15, desc, t.text());
    }
}

/// Side-by-side placement for slot spans sorted by start: `(lane, lanes)`
/// per span, where `lanes` counts the columns its overlap cluster needs, so
/// an event that overlaps nothing keeps the full width.
fn overlap_lanes(spans: &[(u32, u32)]) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(spans.len());
    let mut cluster_start = 0;
    let mut cluster_end = 0;
    let mut lane_end: Vec<u32> = Vec::new();
    for (i, &(a, b)) in spans.iter().enumerate() {
        if a >= cluster_end && i > 0 {
            let n = lane_end.len();
            out[cluster_start..]
                .iter_mut()
                .for_each(|(_, w): &mut (usize, usize)| *w = n);
            cluster_start = i;
            lane_end.clear();
        }
        let lane = lane_end
            .iter()
            .position(|end| *end <= a)
            .unwrap_or_else(|| {
                lane_end.push(0);
                lane_end.len() - 1
            });
        lane_end[lane] = b;
        cluster_end = if i == cluster_start {
            b
        } else {
            cluster_end.max(b)
        };
        out.push((lane, 0));
    }
    let n = lane_end.len();
    out[cluster_start..].iter_mut().for_each(|(_, w)| *w = n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_are_per_overlap_cluster() {
        // standup alone; then a 12-14 block overlapped by 12:30-13:30; then 15-16 alone
        let spans = [(19, 20), (24, 28), (25, 27), (30, 32)];
        assert_eq!(overlap_lanes(&spans), [(0, 1), (0, 2), (1, 2), (0, 1)]);
        // a chain where the third reuses lane 0
        assert_eq!(
            overlap_lanes(&[(0, 4), (2, 6), (4, 8)]),
            [(0, 2), (1, 2), (0, 2)]
        );
        assert!(overlap_lanes(&[]).is_empty());
    }

    #[test]
    fn duration_bars() {
        assert_eq!(duration_bar(15 * 60), "▌");
        assert_eq!(duration_bar(90 * 60), "███");
        assert_eq!(duration_bar(0), "▏");
        assert_eq!(duration_label(90 * 60), "1h30");
    }
}
