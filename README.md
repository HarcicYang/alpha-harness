# alpha-harness

**Harness of your own: Modular, Lightweight, Atomic.**

**Modular**: a small kernel; behavior lives in replaceable crates with declared capabilities.

**Lightweight**: activation, services, events, tasks, and stdio. No JavaScript toolchain or runtime plugin loader.

**Atomic**: the whole composition is built with small crates and validated before linking. Missing or duplicate capabilities fail the build.

## Install

Rust 1.99 or newer is required. Linux desktop builds also need the GPUI system
packages listed in [CONTRIBUTING.md](CONTRIBUTING.md#build-from-source).

From a checkout of this repository:

```sh
cargo install --path apps/alpha-harness
```

For a local build, run `cargo build --release`. The binary is
`target/release/alpha`.

## Configure

Start the desktop app:

```sh
alpha desktop
```

Choose a provider in the app and enter its API key. Provider settings are
stored in `~/.config/alpha-harness/config.toml`, or
`$XDG_CONFIG_HOME/alpha-harness/config.toml` when that variable is set. API keys
stay in the system credential store.

Sessions are stored under `~/.local/state/alpha-harness/sessions`, or
`$ALPHA_HARNESS_STATE_DIR/sessions` when set.

## Run

```sh
alpha tui
alpha run "review the error handling in src"
```

Interactive commands use the current directory as the workspace. `alpha
sessions` lists saved sessions.

## Compose

The composition is the Cargo dependency graph. A composition root lists the
modules, providers, and frontends it contains. `alpha-build` reads that graph,
checks capabilities and metadata, and generates the activation table used by the
binary.

Removing a module removes its commands and transitive dependencies. If another
module still requires a capability that only the removed crate provided, the
build fails and names the missing requirement. Replacing a capability means
swapping one implementation crate for another.

`apps/alpha-min` is a small working composition: the core, one provider kind,
and the headless `run` command. See
[docs/composition-decoupling.md](docs/composition-decoupling.md) for the
metadata fields and replacement rules.

## Documentation

- [Usage](docs/usage.md)
- [Composition and replacement](docs/composition-decoupling.md)
- [Contributing](CONTRIBUTING.md)

## License

MIT
