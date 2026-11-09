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

# change only the given fields; "" clears a field (an occurrence falls back to the series)
vcal edit <id> --start "2026-11-11 09:00"          # moves it, keeps its length
vcal edit <id> --all-day --end 2026-11-12
vcal edit <id> --occurrence <ts> --title "Standup (remote)"
vcal edit <id> --rrule ""                           # stop repeating

# delete an event, or cancel one occurrence of a repeating event
vcal rm <id>
vcal rm <id> --occurrence <ts>

vcal import calendar.ics                            # or - for stdin

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
  "status": "confirmed", "recurring": true, "modified": false }
```

`occurrence` is the handle that `edit --occurrence` and `rm --occurrence`
take; it's `null` for events that don't repeat. All-day rows have dates in
`start`/`end` (end exclusive) and local-midnight timestamps.

For example, the next event's title for a status bar:

```sh
vcal query --from now --to +1d --format json | jq -r '.[0].title // empty'
```
