# vcal

A terminal calendar with Vim modal keys, backed by SQLite. The TUI, a
headless CLI for scripts, and desktop widgets all share one database file,
and every write is visible to the others within a second.

```
vcal                           # the TUI (month view)
vcal --view week --date tomorrow
vcal query --from today --to +7d --format json
```

Source: <https://github.com/beplyn/vcal> (branch `master`).

## Install

With Nix:

```sh
nix run github:beplyn/vcal             # try it
nix profile install github:beplyn/vcal
```

Without Nix, with a Rust toolchain (1.88+) and a C compiler:

```sh
cargo install --locked --git https://github.com/beplyn/vcal
```

[docs/install.md](docs/install.md) covers flake inputs, the overlay, and the
man page and shell completions for cargo installs.

## Documentation

| Topic | |
|---|---|
| [Installing](docs/install.md) | Nix (flake input, overlay, profile) and cargo |
| [The TUI](docs/tui.md) | Views, modes, key bindings, `:` commands, the event dialog |
| [The CLI](docs/cli.md) | `query`, `next`, `add`, `edit`, `rm`, `notify`, `import`, `export`, `config`; JSON output |
| [Configuration](docs/configuration.md) | File locations, `config.toml`, themes, key bindings |
| [Home-Manager](docs/home-manager.md) | `programs.vcal`, Stylix colours |
| [Desktop integration](docs/desktop.md) | Watching the database from widgets and scripts |
| [Development](docs/development.md) | Dev shell, tests, `nix flake check`, the flake's layout |
