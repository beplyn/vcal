# Home-Manager module: installs vcal and writes ~/.config/calendar/config.toml
# from `settings`, with the Base16 palette taken from Stylix when it's there.
self:

{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.vcal;
  toml = pkgs.formats.toml { };

  # Stylix's Home-Manager side sets lib.stylix; plain Home-Manager doesn't
  hasStylix = config.lib ? stylix;
  slots = map (n: "base0${lib.toHexString n}") (lib.range 0 15);
  stylixPalette = lib.genAttrs slots (s: config.lib.stylix.colors.withHashtag.${s});

  generated = lib.foldl' lib.recursiveUpdate { }
  [
    (lib.optionalAttrs (cfg.stylix.enable && hasStylix) { theme = stylixPalette; })
    { general.database_path = cfg.databasePath; }
    # what the user sets wins
    cfg.settings
  ];
in
{
  imports = [ (lib.mkAliasOptionModule [ "programs" "vim-calendar" ] [ "programs" "vcal" ]) ];

  options.programs.vcal =
  {
    enable = lib.mkEnableOption "vcal, a Vim-keybound terminal calendar";

    package = lib.mkOption
    {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "vcal.packages.\${system}.default";
      description = "The vcal package.";
    };

    databasePath = lib.mkOption
    {
      type = lib.types.str;
      default = "${config.xdg.dataHome}/calendar/events.db";
      defaultText = lib.literalExpression ''"''${config.xdg.dataHome}/calendar/events.db"'';
      description = ''
        The SQLite database shared by the TUI, the CLI and anything reading it
        (written to `general.database_path`). `$VCAL_DB` and `--db` still
        override it at runtime.
      '';
    };

    settings = lib.mkOption
    {
      type = toml.type;
      default = { };
      example = lib.literalExpression ''
        {
          general.week_start = "sunday";
          views = { start = "week"; slot_minutes = 15; };
          theme.roles.accent = "base0E";
          keybindings.normal.quit = [ "q" "ZZ" ];
        }
      '';
      description = ''
        Written to {file}`$XDG_CONFIG_HOME/calendar/config.toml`; see
        `vcal config default` for every option. Takes precedence over the
        Stylix palette and {option}`databasePath`.
      '';
    };

    notify =
    {
      enable = lib.mkEnableOption ''
        desktop notifications for event reminders: a systemd user timer runs
        `vcal notify` every minute, which calls `notify-send`. Needs a running
        notification daemon'';

      package = lib.mkPackageOption pkgs "libnotify" { };
    };

    sync =
    {
      enable = lib.mkEnableOption ''
        a systemd user timer that runs `vcal sync`, keeping the `[calendars]`
        of {option}`settings` (feeds, Proton links, CalDAV) in step'';

      interval = lib.mkOption
      {
        type = lib.types.str;
        default = "15min";
        example = "1h";
        description = "How often to sync, as a systemd time span.";
      };

      packages = lib.mkOption
      {
        type = lib.types.listOf lib.types.package;
        default = [ ];
        example = lib.literalExpression "[ pkgs.pass ]";
        description = ''
          Put on the PATH of the sync service, for what `password_command`
          runs (`sh` and coreutils are always there).
        '';
      };
    };

    stylix.enable = lib.mkOption
    {
      type = lib.types.bool;
      default = hasStylix;
      defaultText = lib.literalExpression "true when Stylix is configured";
      description = "Fill `[theme]` with the Base16 colours of the active Stylix scheme.";
    };
  };

  config = lib.mkIf cfg.enable
  {
    assertions =
    [
      {
        assertion = cfg.stylix.enable -> hasStylix;
        message = "programs.vcal.stylix.enable needs Stylix's Home-Manager integration.";
      }
    ];

    home.packages = [ cfg.package ];
    xdg.configFile."calendar/config.toml".source = toml.generate "vcal-config.toml" generated;

    systemd.user = lib.mkMerge
    [
      (lib.mkIf cfg.notify.enable
      {
        services.vcal-notify =
        {
          Unit =
          {
            Description = "Send vcal event reminders";
            After = [ "graphical-session.target" ];
            PartOf = [ "graphical-session.target" ];
          };
          Service =
          {
            Type = "oneshot";
            ExecStart = "${lib.getExe cfg.package} notify";
            Environment = [ "PATH=${lib.makeBinPath [ cfg.notify.package ]}" ];
          };
        };

        timers.vcal-notify =
        {
          Unit =
          {
            Description = "Check vcal reminders every minute";
            PartOf = [ "graphical-session.target" ];
          };
          Timer =
          {
            OnCalendar = "*:*:00";
            AccuracySec = "5s";
          };
          Install.WantedBy = [ "graphical-session.target" ];
        };
      })

      (lib.mkIf cfg.sync.enable
      {
        services.vcal-sync =
        {
          Unit.Description = "Sync vcal's calendars";
          Service =
          {
            Type = "oneshot";
            ExecStart = "${lib.getExe cfg.package} sync";
            Environment =
              [ "PATH=${lib.makeBinPath ([ pkgs.bash pkgs.coreutils ] ++ cfg.sync.packages)}" ];
          };
        };

        timers.vcal-sync =
        {
          Unit.Description = "Sync vcal's calendars every ${cfg.sync.interval}";
          Timer =
          {
            OnStartupSec = "1min";
            OnUnitActiveSec = cfg.sync.interval;
          };
          Install.WantedBy = [ "timers.target" ];
        };
      })
    ];
  };
}
