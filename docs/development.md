# Development

```sh
git clone https://github.com/beplyn/vcal
cd vcal
nix develop        # toolchain, rust-analyzer, sqlite3 (or `direnv allow`)
cargo test         # storage, CLI (as a subprocess), TUI (headless), config
nix flake check    # the above plus clippy, rustfmt, packaged assets, HM module
```

Without Nix, any Rust 1.88+ toolchain and a C compiler work for `cargo
build` and `cargo test`. `nix flake check` additionally runs
`cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`, which
you can run by hand too.

Work happens on `master`.

## Layout

| Path | |
|---|---|
| `src/db/` | SQLite storage, migrations, recurrence expansion |
| `src/tui/` | The ratatui front end: app state, key maps, dialog, drawing |
| `src/cli.rs` | Subcommands, including the hidden `gen-assets` |
| `src/config/` | `config.toml` parsing; `default.toml` is the documented default |
| `src/ics.rs` | iCalendar import |
| `tests/` | Integration tests |
| `flake.nix` | Packages, overlay, Home-Manager module, checks, dev shell |
| `nix/package.nix` | The derivation, as a `callPackage` function |
| `nix/hm-module.nix` | The Home-Manager module |
| `nix/checks.nix` | Checks for the man page, completions and HM module |

## Flake outputs

| Output | |
|---|---|
| `packages.<system>.{vcal,default}` | The program, with man page and completions |
| `overlays.default` | Adds `pkgs.vcal` |
| `homeManagerModules.{default,vcal}` | `programs.vcal` |
| `checks.<system>.*` | build + tests, clippy, fmt, package-assets, hm-* |
| `devShells.<system>.default` | The dev shell |

Inputs: `nixpkgs`, `systems` (which systems to build for), and
`home-manager`, which only the checks use.
