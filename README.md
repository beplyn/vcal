# vcal

A terminal calendar with Vim modal keys, backed by SQLite. The TUI, a
headless CLI for scripts, and desktop widgets all share one database file,
and every write is visible to the others within a second.

```
vcal                           # the TUI (month view)
vcal --view week --date tomorrow
vcal query --from today --to +7d --format json
```

## Install

**Nix (flake):**

```nix
# a local checkout; git+file copies only tracked files (not target/)
inputs.vcal.url = "git+file:///home/you/src/calendar";
# then: inputs.vcal.packages.${system}.default
```

Without a flake: `nix build git+file:///home/you/src/calendar`.

The package includes the man page (`man vcal`) and bash, fish and zsh
completions. The Home-Manager module is described below.

**Elsewhere:** `cargo build --release`. SQLite is compiled in, so the only
requirements are a Rust toolchain (1.88+) and a C compiler. Copy
`target/release/vcal` anywhere on `PATH`; `vcal gen-assets <dir>` writes the
man page and completions if you want them.

## Files

| What | Where | Override |
|---|---|---|
| Events | `$XDG_DATA_HOME/calendar/events.db` | `--db`, `$VCAL_DB`, `general.database_path` |
| Config | `$XDG_CONFIG_HOME/calendar/config.toml` | `--config`, `$VCAL_CONFIG` |
| Change signal | `events.db.changed` next to the database | |

Without `XDG_*` set these fall back to `~/.local/share` and `~/.config`.
The database runs in WAL mode, so readers never block the TUI. vcal
rewrites `events.db.changed` after every write; watch that file rather than
the WAL, which SQLite truncates and deletes.

## The TUI

Three views (Month, Week, Day), a command line, and these modes:

| Mode | Enter with | Leave with |
|---|---|---|
| Normal | (start) | |
| Visual: select days (month) or a time range (week/day) | `v` | `Esc`, `v` |
| Insert: the event dialog | `o`, `O`, `cc` | `Enter` saves, `Esc` discards |
| Command | `:` | `Enter`, `Esc` |
| Search: live filter on title, category, notes, location | `/` | `Enter` keeps the filter, `Esc` clears it |

### Normal mode

| Keys | Action |
|---|---|
| `h` `j` `k` `l` / arrows | Move a day; `j`/`k` move a week in month view, a time slot in week/day |
| `w` `b` | Forward / back a week |
| `}` `{` | Forward / back a month |
| `0` `$` | Start / end of the week |
| `gg` `G` | First / last day of the month (week view: week; day view: first/last slot) |
| `t` `.` | Today, at the current hour |
| `Tab` `S-Tab`, `1` `2` `3` | Next / previous view; Month, Week, Day |
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

There are no counts (`3j`), because `0`–`3` are taken by other actions.

### Visual mode

Movement keys work as in Normal mode. `d`/`x` deletes the selected events,
`y` copies them, `>` / `<` moves them a day (month view) or a slot (week/day
view), and `o` creates an event spanning the selection.

### Commands

`:q` `:wq` `:x` quit (everything is saved as you go, so `:w` is a no-op);
`:jump <when>` (or just `:2026-12-24`); `:today`; `:view month|week|day`;
`:import <file.ics>`; `:noh` clears the search; `:reload` re-reads data and
config.

### The event dialog

Start and end take anything `<when>` accepts (below), e.g. `tomorrow 9:00`.
End also takes a length such as `45m`, and when left empty uses the default
length. For all-day events the end is the last day, inclusive. `Repeat` is
an RFC 5545 rule, e.g. `FREQ=WEEKLY;BYDAY=MO,WE`. Use `Tab`/`S-Tab` to move
between fields and `Space` to toggle all day.

## The CLI

`<when>` accepts `now`, `today`, `tomorrow`, `yesterday` (optionally with
` HH:MM`), relative times (`+2h`, `-3d`, `+1w`), epoch seconds, RFC 3339,
`YYYY-MM-DD` and `"YYYY-MM-DD HH:MM"` (local time).

```sh
# what's on; text (default), json or tsv
vcal query --from today --to +7d --format json

# create; prints the new id
vcal add -t "Dentist" -s "2026-11-10 14:00" -D 45m -c health -l "Dr. Lee"
vcal add -t "Trip" -s 2026-11-20 -e 2026-11-22 --all-day
vcal add -t "Standup" -s "today 09:30" -D 15m -r "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR"

# change only the given fields; "" clears a field (an occurrence falls back to the series)
vcal edit <id> --start "2026-11-11 09:00"          # moves it, keeps its length
vcal edit <id> --all-day --end 2026-11-12
vcal edit <id> --occurrence <ts> --title "Standup (remote)"
vcal edit <id> --rrule ""                           # stop repeating

# delete an event, or cancel one occurrence of a repeating event
vcal rm <id>
vcal rm <id> --occurrence <ts>

vcal import calendar.ics                            # or - for stdin
vcal config path | default | check
```

`query --format json` prints an array of occurrences:

```json
{ "id": "…", "occurrence": 1794299400, "title": "Standup", "description": "",
  "category": "work", "location": null, "all_day": false,
  "start": "2026-11-10T09:30:00+01:00", "end": "2026-11-10T09:45:00+01:00",
  "start_ts": 1794299400, "end_ts": 1794300300,
  "status": "confirmed", "recurring": true, "modified": false }
```

`occurrence` is the handle that `edit --occurrence` and `rm --occurrence`
take; it's `null` for events that don't repeat. All-day rows have dates in
`start`/`end` (end exclusive) and local-midnight timestamps. Errors go to
stderr with a non-zero exit, and stdout carries only the requested output.

## Configuration

The first run writes a commented `config.toml` listing every option at its
default (`vcal config default` prints it). It has these sections:

- `[general]`: `week_start`, `clock` (`24h`/`12h`), `default_duration`,
  `database_path`
- `[views]`: `start`, `slot_minutes` (15/30/60), `first_hour`, `last_hour`,
  `new_event_hour`
- `[theme]`: the Base16 palette, `base00` to `base0F` as `#rrggbb`.
  `[theme.roles]` maps UI parts (accent, today, event, tag, …) to slots.
- `[keybindings.normal]` / `[keybindings.visual]`: `action = ["keys", …]` in
  Vim notation (`gg`, `<C-d>`, `<S-Tab>`, `<lt>`). An entry replaces that
  action's keys, and `[]` unbinds it.

A running TUI applies config changes within a second. If an edit is invalid,
it keeps the previous settings and shows which key is wrong.

## Home-Manager

```nix
{
  imports = [ inputs.vcal.homeManagerModules.default ];
  programs.vcal = {
    enable = true;
    # databasePath = "${config.xdg.dataHome}/calendar/events.db";  # the default
    settings = {
      general.week_start = "sunday";
      views.start = "week";
    };
  };
}
```

With Stylix, `stylix.enable` defaults to on and fills `[theme]` from the
active scheme. `settings` always wins. `programs.vim-calendar` is an alias.

## Desktop (Quickshell)

The dotfiles (`modules/home/desktop/calendar.nix`) wire vcal into the shell:

- **The dashboard** (`Mod+D`, or left-click the waybar clock) shows the
  month with event dots and the picked day's agenda. Click a day to pick it;
  click an event to edit it; `+` adds one. The editor can change a single
  occurrence of a repeating event or the whole series, except the series'
  times (use the TUI for those). Double-click a day, or use the terminal
  button, to open it in the TUI.
- **`Mod+Shift+C`** (niri, sway, mango, Hyprland) or right-clicking the
  waybar clock opens the TUI in wezterm.
- `services/Events.qml` calls `vcal query --format json` for the visible
  weeks, and re-queries 150 ms after `events.db.changed` changes and at
  midnight. It writes through `vcal add` / `edit` / `rm` one call at a time.
  Changes made anywhere appear everywhere.

## Development

```sh
nix develop        # toolchain, rust-analyzer, sqlite3
cargo test         # storage, CLI (as a subprocess), TUI (headless), config
nix flake check    # the above plus clippy, rustfmt, packaged assets, HM module
```
