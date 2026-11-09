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
| `stylix.enable` | on when Stylix is configured | Fill `[theme]` from the active Stylix scheme |

`programs.vim-calendar` is an alias for `programs.vcal`.

## Stylix

With [Stylix](https://github.com/nix-community/stylix)'s Home-Manager
integration present, `[theme]` gets the 16 colours of the system scheme, so
vcal follows theme changes on the next switch. Values in `settings.theme`
still win, and `stylix.enable = false` turns it off.

## Editing the config

The generated file is read-only (a link into the Nix store). Change
`settings` and switch; a running TUI picks up the new file within a second.
