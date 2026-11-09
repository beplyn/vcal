# Desktop integration

vcal has no daemon. Widgets, bars and scripts read and write events through
the [CLI](cli.md) and learn about changes from a signal file.

## The change signal

vcal rewrites `events.db.changed` (next to the database) after every write,
from the TUI, the CLI or an import. Watch that file, not the database or
its WAL, which SQLite truncates and deletes.

```sh
# re-print today's agenda whenever anything changes
db_dir=${XDG_DATA_HOME:-$HOME/.local/share}/calendar
while :; do
  vcal query --from today --to tomorrow
  inotifywait -qq -e close_write,moved_to,create "$db_dir"
done
```

A good pattern for widgets:

1. Query the range you display with `vcal query --format json`.
2. On a change to `events.db.changed`, wait ~150 ms (writes can come in
   bursts) and query again. Also re-query at midnight.
3. Write with `vcal add` / `edit` / `rm`, one call at a time, using the
   `id` and `occurrence` from the JSON.

## Opening the TUI from a widget

Run it in your terminal of choice, optionally at a date:

```sh
foot -e vcal --view day --date 2026-12-24
wezterm start -- vcal --date tomorrow
```

## Quickshell

The author's [dotfiles](https://github.com/beplyn/dotfiles)
(`modules/home/desktop/calendar.nix`) follow this pattern in Quickshell: a
dashboard month view with event dots and an agenda, where clicking adds,
edits and deletes events, plus a waybar clock that opens the TUI. It's a
working example to adapt, not part of this repository.
