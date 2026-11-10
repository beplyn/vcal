# Syncing calendars

vcal keeps its own database, and `vcal sync` brings other calendars into it.
Each one is a `[calendars.<name>]` table in `config.toml`:

| `type` | What it is | Direction |
|---|---|---|
| `ics` | An iCalendar feed: `https://`, `webcal://`, or a file | read-only |
| `proton` | A Proton Calendar share link | read-only |
| `caldav` | A CalDAV calendar: Nextcloud, Radicale, Baïkal, Fastmail, … | both ways |

```sh
vcal sync              # every calendar
vcal sync nextcloud    # just one
vcal calendars         # what's configured, event counts, last sync and errors
```

In the TUI, `:sync` (or `:sync <name>`) does the same. `vcal sync` exits
non-zero when a calendar fails, but carries on with the rest. The
Home-Manager module can run it on a timer (see
[home-manager.md](home-manager.md#sync)).

Every event belongs to one calendar or is local only (no calendar).
`vcal query --format json` and `vcal show` print which one.

## Feeds (`ics`)

```toml
[calendars.holidays]
type = "ics"
url = "https://example.com/holidays.ics"   # webcal:// works too, as does a file path
category = "holiday"                        # for events that come without one
```

A feed is fetched with `If-None-Match`/`If-Modified-Since`, so an unchanged
one costs one small request. When it has changed, the calendar's events
become exactly the feed's: new ones are added, changed ones are replaced, and
ones the feed dropped are removed. Event ids are `<calendar>:<UID>`, so the
same event in two feeds, or in a file you imported, stays separate.

Feed events are read-only. Editing or deleting one says so, because the
next sync would undo the change. To have a version of your own, copy it
(`yy`, then `p`); copies go to your default calendar.

## Proton Calendar (`proton`)

Proton Calendar is end-to-end encrypted and has no CalDAV access (Proton
Bridge covers mail only), so vcal can't write to it. What Proton does offer
is a read-only link to a calendar:

1. In Proton Calendar, open the calendar's settings: **Share → Share with
   anyone → Create link**. Choose "full event details" if you want titles
   and not just busy times.
2. Copy the link into the config:

```toml
[calendars.proton]
type = "proton"
url = "https://calendar.proton.me/api/calendar/v1/url/…/calendar.ics?CacheKey=…&PassphraseKey=…"
```

The link is the key to the calendar, so treat it like a password. vcal
leaves the part after `?` out of every message and of `vcal calendars`. If
you'd rather not keep it in a Nix-managed config (which lands in the
world-readable store), keep `config.toml` out of Home-Manager or put the
calendar in a file only you can read. A revoked or changed link fails with a
note saying so.

Proton refreshes these links on its own schedule (often hourly), so changes
can take a while to show up. To get events from vcal into Proton, export
them (`vcal export events.ics`, or `-c work` for one category) and import the
file in Proton Calendar (**Settings → Import**). Importing the same file again
updates events rather than duplicating them, because vcal keeps each event's
UID.

## CalDAV and Nextcloud (`caldav`)

```toml
[calendars.nextcloud]
type = "caldav"
url = "https://cloud.example.com/remote.php/dav/calendars/alice/personal/"
username = "alice"
password_command = "pass show nextcloud/vcal"   # or password_file = "~/.secrets/nextcloud"
default = true                                   # new events go here
```

- **The URL** is the calendar collection itself. In Nextcloud: Calendar →
  the calendar's `⋯` menu → **Copy private link**. That gives
  `…/remote.php/dav/calendars/<user>/<calendar>/`. Other servers show it in
  their web UI or documentation.
- **The password** never goes in the config. `password_command` runs with
  `sh -c` and `password_file` is read, and vcal uses the first line of
  either. With two-factor login on Nextcloud, make an app password under
  **Settings → Security → Devices & sessions**.
- **`default = true`** sends new events there: the dialog's Calendar field
  starts with it, and `:add`, paste and `vcal add` use it. At most one
  calendar can be the default. Without one, new events are local.
- **`read_only = true`** only pulls.

A sync pushes, then pulls:

1. events deleted here are deleted on the server
2. events created or changed here are uploaded (`PUT` with `If-Match` or
   `If-None-Match`, so nothing on the server is overwritten blindly)
3. the server's list of events and ETags is compared with what vcal last saw,
   new and changed events are downloaded, and events gone from the server are
   removed here

When an event changed on both sides, the server's version wins and the sync
prints a warning naming the event. Repeating events go up and down with their
cancelled and moved occurrences (EXDATE, RECURRENCE-ID), reminders as
VALARMs, and a VTIMEZONE for each zone.

Choosing calendars per event:

```sh
vcal add -t "Dentist" -s "tomorrow 14:00" --calendar nextcloud
vcal edit <id> --calendar nextcloud    # move a local event there
vcal edit <id> --calendar ""           # make it local; the next sync deletes it on the server
```

In the TUI, the event dialog has a Calendar field whenever calendars are
configured.

## Removing a calendar

Take its table out of the config and run `vcal sync` (for all calendars).
A feed's events are removed, since they're only copies. A CalDAV
calendar's events stay as local events, because they may hold changes
that never reached the server.
