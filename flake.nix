{
  description = "vcal: a Vim-keybound terminal calendar backed by SQLite";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems =
      [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      manifest = (lib.importTOML ./Cargo.toml).package;
    in
    {
      packages = forAllSystems (pkgs:
      {
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.vcal;

        vcal = pkgs.rustPlatform.buildRustPackage
        {
          pname = manifest.name;
          inherit (manifest) version;

          src = lib.fileset.toSource
          {
            root = ./.;
            fileset = lib.fileset.unions
            [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          # nixpkgs links its own sqlite via pkg-config; plain `cargo build` bundles one
          nativeBuildInputs = [ pkgs.installShellFiles pkgs.pkg-config ];
          buildInputs = [ pkgs.sqlite ];

          # the binary writes its own man page and completions
          postInstall = lib.optionalString (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
            $out/bin/vcal gen-assets assets
            installManPage assets/vcal.1
            installShellCompletion --cmd vcal \
              --bash assets/vcal.bash \
              --fish assets/vcal.fish \
              --zsh assets/_vcal
          '';

          meta =
          {
            inherit (manifest) description;
            mainProgram = "vcal";
          };
        };
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
      });

      homeManagerModules.default = import ./nix/hm-module.nix self;

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
