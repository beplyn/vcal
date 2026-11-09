{
  description = "vcal: a Vim-keybound terminal calendar backed by SQLite";

  inputs =
  {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    # the systems to build for; override with any nix-systems list
    systems.url = "github:nix-systems/default";
    # only for checking the Home-Manager module (nix/checks.nix)
    home-manager =
    {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      systems,
      home-manager,
    }:
    let
      inherit (nixpkgs) lib;
      forAllSystems = f: lib.genAttrs (import systems) (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # `pkgs.vcal` for any nixpkgs, built against that nixpkgs
      overlays.default = final: _prev: { vcal = final.callPackage ./nix/package.nix { }; };

      packages = forAllSystems (pkgs:
      {
        vcal = pkgs.callPackage ./nix/package.nix { };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.vcal;
      });

      checks = forAllSystems (pkgs:
      let
        vcal = self.packages.${pkgs.stdenv.hostPlatform.system}.vcal;
      in
      {
        # build + `cargo test`
        inherit vcal;

        clippy = vcal.overrideAttrs (old:
        {
          pname = "vcal-clippy";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
          buildPhase = "cargo clippy --offline --all-targets -- -D warnings";
          installPhase = "touch $out";
          doCheck = false;
          postInstall = "";
        });

        fmt = pkgs.runCommandLocal "vcal-fmt" { nativeBuildInputs = [ pkgs.cargo pkgs.rustfmt ]; } ''
          cd ${./.}
          cargo fmt --check
          touch $out
        '';
      }
      // import ./nix/checks.nix { inherit self pkgs home-manager; });

      homeManagerModules =
      {
        default = import ./nix/hm-module.nix self;
        vcal = self.homeManagerModules.default;
      };

      devShells = forAllSystems (pkgs:
      {
        default = pkgs.mkShell
        {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.vcal ];
          packages = with pkgs;
          [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            sqlite # the sqlite3 CLI, for poking at events.db
          ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });
    };
}
