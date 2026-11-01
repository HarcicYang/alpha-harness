# Contributing

## Build from source

The workspace uses Rust 1.99 or newer. `rust-toolchain.toml` selects the
toolchain used by the repository.

The `alpha` binary includes the desktop frontend, so Linux builds need the GPUI
system packages:

```sh
sudo apt-get update
sudo apt-get install -y \
  libasound2-dev libfontconfig1-dev libfreetype-dev libssl-dev libudev-dev \
  libvulkan-dev libwayland-dev libx11-dev libx11-xcb-dev libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxcb1-dev libxkbcommon-dev \
  libxkbcommon-x11-dev pkg-config
```

Build the binary:

```sh
cargo build --release
```

Install it from the workspace:

```sh
cargo install --path apps/alpha-harness
```

The trimmed headless binary is available through `cargo build -p alpha-min`.
See [docs/composition-decoupling.md](docs/composition-decoupling.md) for how
Cargo dependencies select modules and frontends.

## Checks

Run these checks before committing:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The desktop test binaries are large. On a machine with limited memory, run the
tests one link at a time:

```sh
CARGO_BUILD_JOBS=1 cargo test --workspace
```

CI disables test debug information and limits Cargo to two build jobs for the
same reason.

## Changes

Keep each commit to one logical change. Use an English Conventional Commit
subject with no body.

Documentation is written in English. [docs/architecture.md](docs/architecture.md)
is the source of truth for the current architecture. Update the relevant
user-facing page when behavior changes.

Do not add JavaScript, TypeScript, Node.js, npm, pnpm, or JavaScript build tools
to the repository or its shipped dependency chain.
