# Configuration

## Files

| What | Where | Override |
|---|---|---|
| Events | `$XDG_DATA_HOME/calendar/events.db` | `--db`, `$VCAL_DB`, `general.database_path` |
| Config | `$XDG_CONFIG_HOME/calendar/config.toml` | `--config`, `$VCAL_CONFIG` |
| Change signal | `events.db.changed` next to the database | |

Without `XDG_*` set these fall back to `~/.local/share` and `~/.config`.
Overrides are listed strongest first.

The database is plain SQLite in WAL mode, so readers never block the TUI.
Back it up by copying `events.db` while vcal isn't writing, or with
`sqlite3 events.db ".backup backup.db"` at any time.

## config.toml

The first run writes a commented `config.toml` listing every option at its
default; `vcal config default` prints the same file. Delete a line to fall
back to the default. Under Home-Manager the file is generated instead (see
[home-manager.md](home-manager.md)).

A running TUI applies changes within a second (except `database_path`). If
an edit is invalid, it keeps the previous settings and shows which key is
wrong. `vcal config check` validates the file without starting anything.

### `[general]`

| Key | Default | |
|---|---|---|
| `week_start` | `"monday"` | First column of the month and week grids; any weekday name |
| `clock` | `"24h"` | `"24h"` (14:30) or `"12h"` (2:30pm) |
| `default_duration` | `"1h"` | Length of new timed events: `"45m"`, `"1h30m"`; a bare number is minutes |
| `database_path` | `$XDG_DATA_HOME/calendar/events.db` | Absolute, or starting with `~/` |

### `[views]`

| Key | Default | |
|---|---|---|
| `start` | `"month"` | View on startup: `"month"`, `"week"` or `"day"` |
| `slot_minutes` | `30` | Week/day grid granularity: 15, 30 or 60 |
| `first_hour`, `last_hour` | `0`, `24` | Hours the week/day grids cover (first inclusive, last exclusive) |
| `new_event_hour` | `9` | Start hour of events created from the month view |

## Themes

`[theme]` is a Base16 palette, `base00` to `base0F` as `"#rrggbb"`. The
default is Base16 "Default Dark". Any Base16 scheme works; paste its 16
colours in.

`[theme.roles]` maps parts of the UI to palette slots:

| Role | Default | Used for |
|---|---|---|
| `background` | `base00` | Background |
| `panel` | `base01` | Panels, status bar, dialogs |
| `selection` | `base02` | Cursor cell, visual selection |
| `border` | `base03` | Borders, grid lines, days outside the month |
| `muted` | `base04` | Secondary text, time labels, past events |
| `text` | `base05` | Text |
| `danger` | `base08` | Errors, cancelled events, delete prompts |
| `urgent` | `base09` | Events starting within the hour |
| `today` | `base0A` | Today, current-time line |
| `event` | `base0B` | Events, success messages |
| `accent` | `base0D` | Selected day, active tab, focused event |
| `tag` | `base0E` | Categories, recurring marks |

```toml
[theme.roles]
accent = "base0E"
```

## Key bindings

`[keybindings.normal]` and `[keybindings.visual]` map an action to a list
of keys in Vim notation: `"gg"`, `"<C-d>"`, `"<S-Tab>"`, `"<CR>"`,
`"<Esc>"`, `"<Space>"`, and `"<lt>"` for `<`.

- An entry replaces all keys of that action in that mode.
- `[]` unbinds it.
- A key sequence taken from another action moves to this one.
- A binding can't be a prefix of another (no `"g"` next to `"gg"`).

```toml
[keybindings.normal]
quit = ["q", "ZZ"]
week_next = ["w", "<C-f>"]
paste = []
```

`vcal config default` lists every action name with its default keys, and
`?` in the TUI shows what's in effect.
