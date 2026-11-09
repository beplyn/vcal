# `nix flake check` beyond build/clippy/fmt: the packaged man page and
# completions, and the Home-Manager module evaluated through real Home-Manager,
# with and without Stylix.
{
  self,
  pkgs,
  home-manager,
}:

let
  inherit (pkgs) lib;
  vcal = self.packages.${pkgs.stdenv.hostPlatform.system}.vcal;

  # a minimal home with the module and `extra`
  home = extra: home-manager.lib.homeManagerConfiguration
  {
    inherit pkgs;
    modules =
    [
      self.homeManagerModules.default
      {
        home.username = "test";
        home.homeDirectory = "/home/test";
        home.stateVersion = "25.11";
      }
      extra
    ];
  };

  configFile = h: h.config.xdg.configFile."calendar/config.toml".source;

  # Stylix's Home-Manager side as the module sees it: lib.stylix.colors.
  # Slot n gets #00000n, so the generated file shows which slot went where.
  slots = map (n: "base0${lib.toHexString n}") (lib.range 0 15);
  fakeStylix.lib.stylix.colors.withHashtag =
    lib.genAttrs slots (s: "#00000${lib.toLower (lib.substring 5 1 s)}");

  # `vcal config check` on the generated file, then the greps in `script`
  checkConfig = name: h: script: pkgs.runCommandLocal "vcal-${name}" { } ''
    f=${configFile h}
    cat "$f"
    ${lib.getExe vcal} --config "$f" config check
    ${script}
    touch $out
  '';

  vanilla = home
  {
    programs.vcal =
    {
      enable = true;
      settings =
      {
        general.week_start = "sunday";
        keybindings.normal.quit = [ "q" "ZZ" ];
      };
    };
  };

  withStylix = home
  {
    imports = [ fakeStylix ];
    programs.vcal =
    {
      enable = true;
      # what the user sets beats the scheme
      settings.theme.base0D = "#123456";
    };
  };

  # the spec's name for the options
  alias = home { programs.vim-calendar.enable = true; };

  # Stylix asked for where there is none: Home-Manager's assertion check
  # throws as soon as the configuration is read
  noStylix = home
  {
    programs.vcal =
    {
      enable = true;
      stylix.enable = true;
    };
  };
in
{
  package-assets = pkgs.runCommandLocal "vcal-assets"
  {
    nativeBuildInputs = [ pkgs.bash pkgs.fish pkgs.zsh pkgs.gzip ];
  } ''
    man=${vcal}/share/man/man1/vcal.1.gz
    gzip -t "$man"
    zcat "$man" | grep -q '^\.TH vcal 1'
    for sub in query add edit rm import config; do
      zcat "$man" | grep -q "$sub" || { echo "man page lacks $sub"; exit 1; }
    done

    bash_c=${vcal}/share/bash-completion/completions/vcal.bash
    fish_c=${vcal}/share/fish/vendor_completions.d/vcal.fish
    zsh_c=${vcal}/share/zsh/site-functions/_vcal
    bash -n "$bash_c"
    fish --no-execute "$fish_c"
    zsh -n "$zsh_c"
    for f in "$bash_c" "$fish_c" "$zsh_c"; do
      grep -q edit "$f" || { echo "$f lacks the edit subcommand"; exit 1; }
    done
    touch $out
  '';

  hm-vanilla =
    assert lib.assertMsg (lib.elem vcal vanilla.config.home.packages) "vcal isn't installed";
    assert lib.assertMsg (!vanilla.config.programs.vcal.stylix.enable) "Stylix is on without Stylix";
    checkConfig "hm-vanilla" vanilla ''
      grep -qx 'week_start = "sunday"' "$f"
      grep -qx 'database_path = "/home/test/.local/share/calendar/events.db"' "$f"
      grep -qx 'quit = \["q", "ZZ"\]' "$f"
      if grep -q '^\[theme\]' "$f"; then echo "a theme without Stylix"; exit 1; fi
    '';

  hm-stylix =
    assert lib.assertMsg withStylix.config.programs.vcal.stylix.enable "Stylix wasn't picked up";
    checkConfig "hm-stylix" withStylix ''
      grep -qx 'base00 = "#000000"' "$f"
      grep -qx 'base0A = "#00000a"' "$f"
      grep -qx 'base0D = "#123456"' "$f"
    '';

  hm-alias-and-assertion =
    assert lib.assertMsg (lib.elem vcal alias.config.home.packages) "programs.vim-calendar doesn't alias programs.vcal";
    assert lib.assertMsg
      (!(builtins.tryEval noStylix.config.home.username).success)
      "stylix.enable without Stylix should fail an assertion";
    pkgs.runCommandLocal "vcal-hm-alias-and-assertion" { } "touch $out";
}
