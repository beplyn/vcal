# Development

```sh
git clone https://github.com/beplyn/vcal
cd vcal
nix develop        # toolchain, rust-analyzer, sqlite3 (or `direnv allow`)
cargo test         # storage, CLI (as a subprocess), TUI (headless), config, sync, properties
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
| `src/db/` | SQLite storage, migrations, recurrence expansion, sync state |
| `src/tui/` | The ratatui front end: app state, key maps, dialog, drawing |
| `src/cli.rs` | Subcommands, including the hidden `gen-assets` |
| `src/config/` | `config.toml` parsing; `default.toml` is the documented default |
| `src/ics/` | iCalendar reading (`mod.rs`, time zone names in `zones.rs`) and writing (`write.rs`) |
| `src/sync/` | `vcal sync`: HTTP, feeds, CalDAV |
| `src/quick.rs` | Quick-add parsing (`:add`, `vcal add <words>`) |
| `src/maintenance.rs` | `vcal doctor` and `vcal backup` |
| `tests/` | Integration tests; `sync.rs` runs a fake CalDAV server, `properties.rs` uses proptest |
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

## Property tests

`tests/properties.rs` checks, over random input, that the iCalendar, `<when>`,
duration and quick-add parsers never panic (whole files cut and spliced at
random places included), that export → import keeps every field of random
events (escaping, folding, all-day, recurrences with changed and cancelled
occurrences, any time zone), and that daily and weekly series keep their
wall-clock time across DST in zones in both hemispheres and with odd offsets.
Each test runs 200 cases. When one fails, proptest prints the smallest input
it found and saves it under `tests/properties.proptest-regressions`. Commit
that file so the case is replayed from then on.
