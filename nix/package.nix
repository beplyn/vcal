# vcal as a callPackage-able derivation: used by the flake's packages, its
# overlay, and anyone who wants `pkgs.callPackage ./nix/package.nix { }`.
{
  lib,
  stdenv,
  rustPlatform,
  installShellFiles,
  pkg-config,
  sqlite,
  # the man page and bash/fish/zsh completions (needs to run the built binary)
  withAssets ? stdenv.buildPlatform.canExecute stdenv.hostPlatform,
}:

let
  manifest = (lib.importTOML ../Cargo.toml).package;
in
rustPlatform.buildRustPackage
{
  pname = manifest.name;
  inherit (manifest) version;

  src = lib.fileset.toSource
  {
    root = ../.;
    fileset = lib.fileset.unions
    [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../tests
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  # nixpkgs links its own sqlite via pkg-config; plain `cargo build` bundles one
  nativeBuildInputs = [ installShellFiles pkg-config ];
  buildInputs = [ sqlite ];

  # the binary writes its own man page and completions
  postInstall = lib.optionalString withAssets ''
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
    homepage = "https://github.com/beplyn/vcal";
    mainProgram = "vcal";
    platforms = lib.platforms.unix;
  };
}
