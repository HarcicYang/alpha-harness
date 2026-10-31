# Contributing

## Build from source

The workspace requires Rust 1.99 or newer; `rust-toolchain.toml` selects the
stable toolchain.

The `alpha` binary always links the desktop front, so every build needs the GPUI
system packages on Linux:

```sh
sudo apt-get update
sudo apt-get install -y \
  libasound2-dev libfontconfig1-dev libfreetype-dev libssl-dev libudev-dev \
  libvulkan-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxcb1-dev libxkbcommon-dev \
  libxkbcommon-x11-dev pkg-config
```

Then build or install:

```sh
cargo build --release
cargo install --path apps/alpha-harness
```

## Checks

Run the repository checks before submitting changes:

```sh
cargo check --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs those checks on Linux and `cargo check --workspace --all-targets` on
macOS and Windows.

## Commits

Use an English Conventional Commit subject with no body, keep each commit scoped
to one logical change, and commit after each small, self-contained change.
[`AGENTS.md`](AGENTS.md) holds the full repository rules.

## Architecture

[`docs/architecture.md`](docs/architecture.md) is the single source of truth for
the architecture. User-facing reference material lives in
[`docs/usage.md`](docs/usage.md).
