# The CLI

The subcommands work without a terminal UI, for scripts, status bars and
widgets. `vcal --help` and `vcal <command> --help` list every flag, and
`man vcal` has the same text.

Errors go to stderr with a non-zero exit, and stdout carries only the
requested output.

## Dates and times

`<when>` accepts:

- `now`, `today`, `tomorrow`, `yesterday`, optionally followed by ` HH:MM`
- relative times: `+2h`, `-3d`, `+1w`
- epoch seconds
- RFC 3339 (`2026-11-10T14:00:00+01:00`)
- `YYYY-MM-DD` and `"YYYY-MM-DD HH:MM"`, in local time

Durations (`-D`, `default_duration`) look like `45m`, `1h`, `1h30m`.

## Commands

```sh
# what's on; text (default), json or tsv
vcal query --from today --to +7d --format json
vcal query --from today --to +30d --search dentist -c health   # filter like / in the TUI

# create; prints the new id
vcal add -t "Dentist" -s "2026-11-10 14:00" -D 45m -c health -l "Dr. Lee"
vcal add -t "Trip" -s 2026-11-20 -e 2026-11-22 --all-day
vcal add -t "Standup" -s "today 09:30" -D 15m -r "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR"
vcal add -t "Flight" -s "2026-11-12 07:10" -R 2h       # remind 2 hours before
vcal add Dentist tomorrow 14:00 45m @health !1h      # the same in words; see tui.md#quick-add

# change only the given fields; "" clears a field (an occurrence falls back to the series)
vcal edit <id> --start "2026-11-11 09:00"          # moves it, keeps its length
vcal edit <id> --all-day --end 2026-11-12
vcal edit <id> --occurrence <ts> --title "Standup (remote)"
vcal edit <id> --rrule ""                           # stop repeating
vcal edit <id> --reminder ""                        # no reminder
vcal edit <id> --timed --start "tomorrow 10:00"     # all-day → timed
vcal edit <id> --status tentative                   # confirmed, tentative or cancelled

# delete an event; for a repeating event that is every occurrence
vcal rm <id>
# or cancel just one occurrence (the `occurrence` value from `query`)
vcal rm <id> --occurrence <ts>

vcal import calendar.ics                            # or - for stdin
vcal sync                                           # fetch feeds, sync CalDAV; see sync.md
vcal calendars                                      # the configured calendars and their last sync
vcal export backup.ics                              # or to stdout; -c work, --id <id> to filter

vcal show <id>                                      # one event in full: recurrence, exceptions, next dates
vcal show <id> --json --upcoming 10                 # the same as JSON, with ten upcoming dates
vcal next                                           # "14:00 Dentist", for a status bar
vcal notify                                         # send reminders that are due

vcal config path       # where the config file is
vcal config default    # the documented default config
vcal config check      # validate it without starting the TUI
```

Every command takes `--db <path>` and `--config <path>` (or `$VCAL_DB` and
`$VCAL_CONFIG`); see [configuration.md](configuration.md#files). With no
command, `vcal` starts the TUI; `--view month|week|day|agenda` and
`--date <when>` pick where it opens.

A few details the examples leave out:

- `add` and `edit` take `--status confirmed|tentative|cancelled` and
  `--calendar <name>` (see [sync.md](sync.md#caldav-and-nextcloud-caldav)).
- `edit` changes only the options you pass. `--start` on its own moves the
  event and keeps its length; `--occurrence <ts>` limits the change to one
  occurrence, and `""` then falls back to the series' value.
- `query` leaves cancelled events and occurrences out unless you pass
  `--include-cancelled`. `--to` is exclusive, but a bare date includes that
  whole day. `--format tsv` prints a header and the columns `id`,
  `occurrence`, `start`, `end`, `all_day`, `status`, `category`, `title`.
- `rm` and `edit` refuse events from read-only calendars (feeds, Proton,
  `read_only = true`), since the next sync would undo the change.
- `sync` takes calendar names to sync only those.

## JSON output

`query --format json` prints an array of occurrences:

```json
{ "id": "…", "occurrence": 1794299400, "title": "Standup", "description": "",
  "category": "work", "location": null, "all_day": false,
  "start": "2026-11-10T09:30:00+01:00", "end": "2026-11-10T09:45:00+01:00",
  "start_ts": 1794299400, "end_ts": 1794300300,
  "status": "confirmed", "reminder": 10, "recurring": true, "modified": false,
  "calendar": "nextcloud" }
```

`calendar` is the `[calendars]` entry the event syncs with, or `null` for a local event (see [sync.md](sync.md)).

`reminder` is minutes before the start, or `null`. `occurrence` is the handle that `edit --occurrence` and `rm --occurrence`
take; it's `null` for events that don't repeat. All-day rows have dates in
`start`/`end` (end exclusive) and local-midnight timestamps.

For example, the next event's title for a status bar:

```sh
vcal query --from now --to +1d --format json | jq -r '.[0].title // empty'
```

## Importing and exporting

`vcal import` keeps each event's UID as its id, so importing a newer copy of
the same file updates events in place. The file wins for the events it
contains: their cancelled and moved occurrences are replaced by the file's.

Time zones can be IANA names (`Europe/Berlin`), Windows names from Outlook
and Exchange (`W. Europe Standard Time`), Outlook display names
(`(UTC+01:00) Amsterdam, Berlin, …`), path-prefixed names
(`/mozilla.org/…/Europe/Berlin`), or custom names that the file describes in a
`VTIMEZONE`. When vcal has to guess a zone from a `VTIMEZONE`'s offsets, or
can't place it at all and falls back to local time, it prints a warning that
names the zone.

`vcal export [FILE]` writes events as iCalendar, to stdout or to FILE
(`:export` or `:w <file>` in the TUI). With no filter it writes every event;
`-c/--category` and `--id` narrow it down, and both can be repeated. Repeating
events keep their RRULE, cancelled occurrences become EXDATEs, changed ones
become RECURRENCE-ID overrides, and reminders become VALARMs. Each time zone
used gets a `VTIMEZONE`, so readers that don't know IANA names still get the
times right. Its transitions run 15 years past today. Importing the file
back, into vcal or anything else, gives the same calendar, so it also works
as a backup format.

## Status bars: `vcal next`

`vcal next` prints the next timed event in the coming week, or nothing (with
exit status 0) when there isn't one, so a bar module can show its output as is.

```sh
vcal next                                       # 14:00 Dentist  /  tomorrow 9:30 Standup
vcal next --format '{in}: {title} ({location})' # in 25m: Dentist (Dr. Lee)
vcal next --format '{start:%a %H:%M} {title}'   # strftime after the colon
vcal next --ongoing                             # count one that has started as next
vcal next --all-day --within 1d                 # include all-day events; look 1 day ahead
vcal next --json                                # one object, the fields of `query`
vcal next --waybar                              # JSON for a waybar module: text, tooltip, class
```

Template fields: `{title}`, `{location}`, `{category}`, `{description}`,
`{id}`, `{start}` / `{end}` (in your 12/24-hour format, or a strftime format as
in `{start:%H:%M}`), `{when}` (`14:00`, `tomorrow 9:30`, `Thu 9:30`) and
`{in}` (`in 25m`, or `now` once started). Write `{{` and `}}` for braces.

A waybar module:

```json
"custom/next-event": {
  "exec": "vcal next --format '{when} {title}'",
  "interval": 60
}
```

`--waybar` prints the JSON that waybar's `"return-type": "json"` reads.
`text` is the `--format` output, `tooltip` lists the next ten events, and
`class` is `ongoing`, `soon` (it starts within `--soon`, 15 minutes by
default), `upcoming`, or `none`. With nothing ahead, `text` is empty and
waybar hides the module. Text is escaped for Pango markup.

```json
"custom/next-event": {
  "exec": "vcal next --waybar --ongoing --format '{when} {title}'",
  "return-type": "json",
  "interval": 60
}
```

```css
#custom-next-event.soon    { color: @base09; }
#custom-next-event.ongoing { color: @base0B; }
```

## Reminders

An event with a reminder (`add -R 10m`, `edit --reminder 1h`, the dialog's
Reminder field, or a `VALARM` with a `TRIGGER` relative to the start in an
imported file) gets a desktop notification that long before it starts.

`vcal notify` sends the ones that are due through `notify-send` and records
them, so each occurrence notifies once. It is meant to run every minute: the
Home-Manager module's `notify.enable` sets up a systemd user timer for it
(see [home-manager.md](home-manager.md)). A reminder still goes out up to 15
minutes after the event starts, for when the machine was asleep. Then it's
skipped. `vcal notify --dry-run` prints what it would send without sending or
recording anything.

## Maintenance: `vcal doctor` and `vcal backup`

`vcal doctor` checks the database (schema version, WAL mode, SQLite's
integrity and foreign-key checks, the change file), every event (time zone,
recurrence rule, all-day bounds, changed or cancelled occurrences that no
longer match their series), the config file, and whether `notify-send` is
available when events have reminders. It prints one `ok`, `warn` or `FAIL`
line per check and exits non-zero if anything failed. It still runs when the
config is broken, so it can report that too.

`vcal backup` writes a consistent copy of the database with SQLite's
`VACUUM INTO`, which is safe while the TUI or a widget has it open. By
default the copy goes to `backups/events-<UTC time>.db` next to the database.
Give a file or a directory to write it somewhere else. `--keep N` then
deletes all but the newest N `events-*.db` files in that directory, so a
daily timer can run `vcal backup --keep 14`.

To restore, quit vcal and copy a backup over the database, or merge one in
without replacing anything:

```sh
vcal --db ~/.local/share/calendar/backups/events-20261110T120000Z.db export | vcal import -
```
