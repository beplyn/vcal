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

# create; prints the new id
vcal add -t "Dentist" -s "2026-11-10 14:00" -D 45m -c health -l "Dr. Lee"
vcal add -t "Trip" -s 2026-11-20 -e 2026-11-22 --all-day
vcal add -t "Standup" -s "today 09:30" -D 15m -r "FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR"
vcal add -t "Flight" -s "2026-11-12 07:10" -R 2h       # remind 2 hours before

# change only the given fields; "" clears a field (an occurrence falls back to the series)
vcal edit <id> --start "2026-11-11 09:00"          # moves it, keeps its length
vcal edit <id> --all-day --end 2026-11-12
vcal edit <id> --occurrence <ts> --title "Standup (remote)"
vcal edit <id> --rrule ""                           # stop repeating
vcal edit <id> --reminder ""                        # no reminder

# delete an event, or cancel one occurrence of a repeating event
vcal rm <id>
vcal rm <id> --occurrence <ts>

vcal import calendar.ics                            # or - for stdin

vcal next                                           # "14:00 Dentist", for a status bar
vcal notify                                         # send reminders that are due

vcal config path       # where the config file is
vcal config default    # the documented default config
vcal config check      # validate it without starting the TUI
```

Every command takes `--db <path>` and `--config <path>`; see
[configuration.md](configuration.md#files).

## JSON output

`query --format json` prints an array of occurrences:

```json
{ "id": "…", "occurrence": 1794299400, "title": "Standup", "description": "",
  "category": "work", "location": null, "all_day": false,
  "start": "2026-11-10T09:30:00+01:00", "end": "2026-11-10T09:45:00+01:00",
  "start_ts": 1794299400, "end_ts": 1794300300,
  "status": "confirmed", "reminder": 10, "recurring": true, "modified": false }
```

`reminder` is minutes before the start, or `null`. `occurrence` is the handle that `edit --occurrence` and `rm --occurrence`
take; it's `null` for events that don't repeat. All-day rows have dates in
`start`/`end` (end exclusive) and local-midnight timestamps.

For example, the next event's title for a status bar:

```sh
vcal query --from now --to +1d --format json | jq -r '.[0].title // empty'
```

## Importing

`vcal import` keeps each event's UID as its id, so importing a newer copy of
the same file updates events in place. The file wins for the events it
contains: their cancelled and moved occurrences are replaced by the file's.

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
