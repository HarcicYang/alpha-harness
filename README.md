# alpha-harness

**Harness of your own.** Modular, lightweight, atomic.

- **Modular** — the feature set is the Cargo dependency graph. Tools, providers,
  prompts, permissions, and compaction are modules with declared requirements
  and provisions, checked at build time: no plugin runtime, no dynamic loading,
  no WASM.
- **Lightweight** — the kernel holds only the component runtime and terminal and
  stdio plumbing. The desktop app, the terminal UI, and the CLI are one binary
  with no JavaScript in the build.
- **Atomic** — nothing is installed or patched in place. The module set is
  resolved from the locked dependency graph and baked into the binary at build
  time, so a capability cannot be half-installed; repair and migration write a
  successor session generation beside the untouched source file.

## Install

```sh
cargo install --path apps/alpha-harness
```

Building requires Rust 1.99 or newer, and the installed binary is named
`alpha`. On Linux the build also needs the system packages listed in
[CONTRIBUTING.md](CONTRIBUTING.md#build-from-source). To build without
installing, run `cargo build --release`; the binary lands in
`target/release/alpha`.

## Quick start

```sh
alpha desktop
```

On first use, add a provider — OpenAI, Anthropic, DeepSeek, OpenCode Go, or any
compatible endpoint — together with its API key. Then choose a workspace from
the top-left menu and describe the task in the composer.

From a terminal:

```sh
alpha run "review the error handling in src"
```

Assistant text goes to stdout; progress, tool calls, errors, and the final
session ID go to stderr.

## Commands

| Command | What it does |
| --- | --- |
| `alpha run [OPTIONS] [PROMPT]...` | Run one prompt and exit |
| `alpha chat` | Chat without switching to the alternate screen |
| `alpha tui` | Open the full-screen terminal interface |
| `alpha desktop` | Open the desktop app |
| `alpha sessions` | List stored sessions |

`alpha chat`, `alpha tui`, and `alpha desktop` use the current directory as the
workspace. Only `alpha run` can be pointed at another directory.

### `alpha run` options

| Option | What it does |
| --- | --- |
| `--workspace <PATH>` | Choose the workspace. The current directory is the default. |
| `--mode <MODE>` | Choose `read`, `auto`, `reviewed`, or `free`. Defaults to `auto`. |
| `--provider <ID>` | Choose a provider instance or provider kind. Defaults to the configured default instance. |
| `--model <MODEL>` | Choose a model. Defaults to the provider catalog. |
| `--resume <SESSION_ID>` | Continue a saved session instead of starting a new one |

```sh
alpha run --mode free "fix the failing tests"
alpha run --resume <SESSION_ID> "continue"
```

A prompt is required unless `--resume` is given.

## Permission modes

`auto` is the default. Interactive fronts show the active mode; `alpha run`
takes it from `--mode`.

- `read` allows reading only. Every mutation is denied.
- `auto` allows sandboxed work without prompting; anything else needs approval.
- `reviewed` also routes writes through an automated reviewer.
- `free` allows everything.

Prompts offer deny, allow once, and allow for the session, and session grants
end with the session. See
[`docs/architecture.md`](docs/architecture.md#permissions) for the full matrix
and policy rules.

## Files

Provider settings live in `~/.config/alpha-harness/config.toml`; saved sessions
live in `~/.local/state/alpha-harness/sessions`. Both locations honour
`XDG_CONFIG_HOME`, `XDG_STATE_HOME`, and `ALPHA_HARNESS_STATE_DIR`; see
[`docs/usage.md`](docs/usage.md#files-and-environment) for the exact order.

API keys never enter the configuration file. They go to the system credential
store and the file keeps only an opaque reference.

## Documentation

- [`docs/usage.md`](docs/usage.md) — terminal keys, slash commands, desktop
  shortcuts, sandbox behavior, file locations.
- [`docs/architecture.md`](docs/architecture.md) — how the agent, modules, and
  frontends fit together.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — building from source, checks, commits.

## License

MIT. See [`LICENSE`](LICENSE).
