# The TUI

```sh
vcal                              # month view, today
vcal --view week --date tomorrow  # any <when> from cli.md
```

Three views (Month, Week, Day), a command line, and these modes:

| Mode | Enter with | Leave with |
|---|---|---|
| Normal | (start) | |
| Visual: select days (month) or a time range (week/day) | `v` | `Esc`, `v` |
| Insert: the event dialog | `o`, `O`, `cc` | `Enter` saves, `Esc` discards |
| Command | `:` | `Enter`, `Esc` |
| Search: live filter on title, category, notes, location | `/` | `Enter` keeps the filter, `Esc` clears it |

Every key below can be rebound; see
[configuration.md](configuration.md#key-bindings). `?` shows the bindings
currently in effect.

## Normal mode

| Keys | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move a day; `j`/`k` move a week in month view, a time slot in week/day |
| `w` `b` | Forward / back a week |
| `}` `{` | Forward / back a month |
| `0` `$` | Start / end of the week |
| `gg` `G` | First / last day of the month (week view: week; day view: first/last slot) |
| `t` `.` | Today, at the current hour |
| `Tab` `S-Tab`, `g1` `g2` `g3` | Next / previous view; Month, Week, Day |
| `Enter` | Focus the day's event list (`j`/`k` pick one); `Enter` again edits it |
| `o` / `O` | New event at the cursor / new all-day event |
| `dd` | Delete (asks to confirm; for a repeating event, asks: this occurrence or all) |
| `cc` | Edit (same choice for repeating events) |
| `yy` / `p` | Copy the event / paste a copy at the cursor |
| `/`, `n` `N` | Search; next / previous match (up to a year away) |
| `:` | Command line |
| `?` | All key bindings |
| `Esc` `C-[` `C-c` | Back out: list focus, selection, filter |
| `ZZ` | Quit |

A count repeats a motion, as in Vim: `3l`, `2w`, `10j`, `3}`, `2n`. It also
works for `>`/`<` in visual mode (`v3>`). `0` on its own is still start of the
week; other actions ignore a count. Counts go up to 999.

## Visual mode

Movement keys work as in Normal mode. `d`/`x` deletes the selected events,
`y` copies them, `>` / `<` moves them a day (month view) or a slot (week/day
view), and `o` creates an event spanning the selection.

## Commands

| Command | |
|---|---|
| `:q` `:wq` `:x` | Quit. Everything is saved as you go, so `:w` is a no-op |
| `:jump <when>`, or just `:2026-12-24` | Go to a date |
| `:today` | Go to today |
| `:view month\|week\|day` | Switch view |
| `:import <file.ics>` | Import an iCalendar file |
| `:add <words>`, `:a` | Create an event from one line: `:add Dentist tomorrow 14:00 45m @health` ([quick add](#quick-add)) |
| `:export <file.ics>`, `:w <file.ics>` | Write every event to an iCalendar file |
| `:noh` | Clear the search filter |
| `:reload` | Re-read events and config |

### Quick add

`:add` (and `vcal add` given words instead of `--title`/`--start`) reads one
line. The words it recognises are taken out, and the rest, in order, is the
title:

| Words | Meaning |
|---|---|
| `today` `tomorrow` `fri` `friday` `2026-11-20` `11-20` `20 nov` `nov 20` | The day. A weekday means the next one, today included. Default: the cursor's day (`vcal add`: today) |
| `14:00` `9:30am` `2pm` `14:00-15:30` | Start time, or start and end. Without one the event is all-day |
| `45m` `1h30m` `2d` | Length (whole days for all-day events) |
| `@health` | Category |
| `!10m` | Reminder that long before the start |
| `daily` `weekly` `monthly` `yearly` `every mon,wed` `every weekday` `every day` | Repeat |
| `\word` | A word kept in the title as is (`\today`) |

```
:add Dentist tomorrow 14:00 45m @health !1h
:add Standup mon 9:30-9:45 every weekday @work
:add Trip 2026-11-20 3d
```

## The event dialog

Start and end take anything `<when>` accepts (see [cli.md](cli.md#dates-and-times)),
e.g. `tomorrow 9:00`. End also takes a length such as `45m`, and when left
empty uses the default length. For all-day events the end is the last day,
inclusive. `Repeat` is an RFC 5545 rule, e.g. `FREQ=WEEKLY;BYDAY=MO,WE`.
`Reminder` is how long before the start `vcal notify` should alert you
(`10m`, `1h`, `1d`); leave it empty for none. Use `Tab`/`S-Tab` to move
between fields and `Space` to toggle all day.

## Shared with other programs

The TUI notices writes from the CLI and from desktop widgets within a
second and redraws, so there's no need to restart it. See
[desktop.md](desktop.md).
