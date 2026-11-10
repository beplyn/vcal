# Home-Manager

The flake ships a module that installs vcal and generates
`$XDG_CONFIG_HOME/calendar/config.toml`. Add the flake as an input first
(see [install.md](install.md#as-a-flake-input)).

```nix
{ config, inputs, ... }:
{
  imports = [ inputs.vcal.homeManagerModules.default ];

  programs.vcal = {
    enable = true;
    # databasePath = "${config.xdg.dataHome}/calendar/events.db";  # the default
    settings = {
      general.week_start = "sunday";
      views = { start = "week"; slot_minutes = 15; };
      theme.roles.accent = "base0E";
      keybindings.normal.quit = [ "q" "ZZ" ];
    };
  };
}
```

## Options

| Option | Default | |
|---|---|---|
| `enable` | `false` | Install vcal and write the config |
| `package` | this flake's `vcal` | Set to `pkgs.vcal` if you use the [overlay](install.md#as-a-flake-input) |
| `databasePath` | `${config.xdg.dataHome}/calendar/events.db` | Written to `general.database_path`; `$VCAL_DB` and `--db` still override it |
| `settings` | `{ }` | Any `config.toml` content (see [configuration.md](configuration.md)); wins over everything else |
| `notify.enable` | `false` | A systemd user timer that runs `vcal notify` every minute (see [cli.md](cli.md#reminders)) |
| `notify.package` | `pkgs.libnotify` | Provides the `notify-send` it calls |
| `sync.enable` | `false` | A systemd user timer that runs `vcal sync` (see [below](#sync)) |
| `sync.interval` | `"15min"` | How often, as a systemd time span |
| `sync.packages` | `[ ]` | Added to the sync service's `PATH` for `password_command` (`sh` and coreutils are always there) |
| `stylix.enable` | on when Stylix is configured | Fill `[theme]` from the active Stylix scheme |

`programs.vim-calendar` is an alias for `programs.vcal`.

## Sync

Calendars go in `settings.calendars` (every key is in
[sync.md](sync.md)), and `sync.enable` keeps them in step:

```nix
programs.vcal = {
  enable = true;
  sync = { enable = true; interval = "15min"; packages = [ pkgs.pass ]; };
  settings.calendars = {
    nextcloud = {
      type = "caldav";
      url = "https://cloud.example.com/remote.php/dav/calendars/alice/personal/";
      username = "alice";
      password_command = "pass show nextcloud/vcal";
      default = true;
    };
    holidays = { type = "ics"; url = "https://example.com/holidays.ics"; category = "holiday"; };
  };
};
```

The generated config lands in the Nix store, which anyone on the machine can
read. Keep passwords out of it (use `password_command` or `password_file`),
and think twice before putting a Proton share link there, since the link is
the secret. `journalctl --user -u vcal-sync` shows how each run went, and
`vcal calendars` shows each calendar's last result.

## Stylix

With [Stylix](https://github.com/nix-community/stylix)'s Home-Manager
integration present, `[theme]` gets the 16 colours of the system scheme, so
vcal follows theme changes on the next switch. Values in `settings.theme`
still win, and `stylix.enable = false` turns it off.

## Editing the config

The generated file is read-only (a link into the Nix store). Change
`settings` and switch; a running TUI picks up the new file within a second.
