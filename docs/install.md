# Installing vcal

The repository is <https://github.com/beplyn/vcal>; `master` is the
default branch and what every command below builds.

- [With Nix](#with-nix)
- [Without Nix (cargo)](#without-nix-cargo)
- [Uninstalling](#uninstalling)

## With Nix

The flake builds for Linux and macOS (the systems in
[nix-systems/default](https://github.com/nix-systems/default)). The package
includes the man page (`man vcal`) and bash, fish and zsh completions.

### Try it, or install it into a profile

```sh
nix run github:beplyn/vcal
nix run github:beplyn/vcal -- query --from today --to +7d

nix profile install github:beplyn/vcal
nix profile upgrade vcal        # later, to pull the newest master
```

A specific commit or tag: `github:beplyn/vcal/<rev>`.

### As a flake input

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    vcal = {
      url = "github:beplyn/vcal";
      # share your nixpkgs instead of fetching a second one
      inputs.nixpkgs.follows = "nixpkgs";
      # the flake's home-manager input is only used by its own checks;
      # point it at yours (or anything) so it isn't fetched separately
      inputs.home-manager.follows = "home-manager";
      # optional: only the systems you build for
      # inputs.systems.url = "github:nix-systems/x86_64-linux";
    };
  };
}
```

The flake input pins a commit in your `flake.lock`. To pick up new commits
on `master`, run `nix flake update vcal`.

Then use one of:

```nix
# the package directly
environment.systemPackages = [ inputs.vcal.packages.${pkgs.system}.default ];  # NixOS
home.packages = [ inputs.vcal.packages.${pkgs.system}.default ];               # Home-Manager
```

```nix
# the overlay: `pkgs.vcal`, built against *your* nixpkgs
nixpkgs.overlays = [ inputs.vcal.overlays.default ];
environment.systemPackages = [ pkgs.vcal ];
```

nixpkgs has an unrelated package that is also called `vcal`. The overlay
replaces it.

```nix
# the Home-Manager module: the package plus a generated config.toml
imports = [ inputs.vcal.homeManagerModules.default ];
programs.vcal.enable = true;
```

See [home-manager.md](home-manager.md) for the module's options.

### Customising the package

`nix/package.nix` is an ordinary `callPackage` function, so the usual
override mechanisms work:

```nix
# skip the man page and completions (e.g. when cross-compiling; this is
# already the default when the build machine can't run the binary)
inputs.vcal.packages.${pkgs.system}.vcal.override { withAssets = false; }

# or build it from a checkout with any nixpkgs
pkgs.callPackage ./vcal/nix/package.nix { }
```

### Without flakes

```sh
nix-build -E 'with import <nixpkgs> { }; callPackage (fetchTarball "https://github.com/beplyn/vcal/archive/master.tar.gz" + "/nix/package.nix") { }'
```

## Without Nix (cargo)

### Requirements

- A Rust toolchain, **1.88 or newer** (`rustup default stable` is enough).
- A C compiler (`cc`, `gcc` or `clang`). SQLite is compiled into the binary,
  so you don't need SQLite installed.
- Linux, macOS or another Unix. vcal reads the system time zone, so make
  sure `/etc/localtime` or `$TZ` is set.

On Debian/Ubuntu `apt install build-essential`, on Fedora
`dnf install gcc`, on Arch `pacman -S base-devel`, and on macOS
`xcode-select --install`.

### Install from GitHub

```sh
cargo install --locked --git https://github.com/beplyn/vcal
```

This builds the `master` branch and puts `vcal` into `~/.cargo/bin`, which
needs to be on your `PATH`. `--locked` uses the exact dependency versions
the project is tested with. Run the same command with `--force` to update.

A specific commit or tag: add `--rev <sha>` or `--tag <tag>`.

### Install from a checkout

```sh
git clone https://github.com/beplyn/vcal
cd vcal
cargo install --locked --path .
# or build without installing; the binary is target/release/vcal
cargo build --release --locked
```

### Man page and shell completions

The binary generates them itself:

```sh
vcal gen-assets ~/.cache/vcal-assets
```

That writes `vcal.1`, `vcal.bash`, `vcal.fish` and `_vcal` (zsh). Copy them
where your system looks:

```sh
d=~/.cache/vcal-assets

# man page
install -Dm644 $d/vcal.1 ~/.local/share/man/man1/vcal.1

# bash (needs the bash-completion package)
install -Dm644 $d/vcal.bash ~/.local/share/bash-completion/completions/vcal

# fish
install -Dm644 $d/vcal.fish ~/.config/fish/completions/vcal.fish

# zsh: any directory on $fpath, e.g. with this in ~/.zshrc:
#   fpath=(~/.zfunc $fpath); autoload -Uz compinit; compinit
install -Dm644 $d/_vcal ~/.zfunc/_vcal
```

Regenerate them after updating vcal, since new options show up there.

### First run

```sh
vcal                 # creates the database and a commented config.toml
vcal config path     # where the config went
```

See [configuration.md](configuration.md) for what goes in it.

## Uninstalling

```sh
nix profile remove vcal       # Nix profile
cargo uninstall vcal          # cargo
```

Your data stays in `$XDG_DATA_HOME/calendar/` (usually
`~/.local/share/calendar/`) and the config in `$XDG_CONFIG_HOME/calendar/`.
Delete those directories to remove everything.
