# alpha-harness

A lightweight, everything-is-a-module coding agent harness. Design decisions
live in [docs/architecture.md](docs/architecture.md).

## Layout

- `apps/alpha-harness`: the `alpha` binary.
- `crates/`: kernel, protocol, and modules.

## Build

```sh
cargo build --release
```

The workspace requires Rust 1.88 or newer.

## Run one prompt

```sh
alpha run --mode free "add a README section"
```

- `--mode read|auto|reviewed|free` selects the permission mode (`auto` is the
  default).
- `--workspace <path>` selects the workspace root; it defaults to the current
  directory.
- `--provider <ID>` selects a configured provider instance; otherwise the
  configured default is used.
- `--model <MODEL>` selects a model; otherwise the provider catalog default is
  used.
- Provider credentials are stored in the platform keyring and configured from
  `alpha tui`. Environment credentials are not used.
- Interactive terminals get approval prompts in `auto` mode. Non-interactive
  runs fail closed and need `--mode free` or explicit policy rules.
- Shell commands run inside a bubblewrap sandbox when it is available.
- Assistant output streams to stdout as the model produces it. Step, tool, and
  error progress stays on stderr.

## Terminal interface

```sh
alpha tui
```

The TUI starts its own core process, discovers provider kinds and persisted
provider instances, and exposes provider creation, removal, default selection,
credential entry, validation, schema-guided JSON configuration editing, and
refresh. Secrets are written to the platform keyring by the core and are never
returned to the TUI.

The chat header always shows the effective provider, model, thinking level, and
permission mode. `Ctrl-L` opens the model picker, `Shift-Tab` cycles thinking
levels, and `Ctrl-T` opens the thinking-level picker. The equivalent commands
are `/model`, `/thinking`, and `/effort`. Route changes are recorded in the
session log and restored when the session is resumed. Model selection also
becomes the persisted default for new sessions, and model pickers prefer the
provider's live `/models` catalog over bundled fallback metadata.

## Prompting and context

- `AGENTS.md` and `.alpha/AGENTS.md` files are discovered from the workspace
  root through the fixed cwd and added to the system prompt.
- The complete prompt is recorded in the session log and replayed on resume.
- Long histories are compacted into a durable summary before the configured
  context budget is exhausted. Compaction failures leave the original history
  untouched.

## Sessions

```sh
alpha sessions                        # list stored sessions
alpha run --resume <SESSION_ID>       # reopen idle and repair interrupted work
alpha run --resume <SESSION_ID> "continue"
```

## Common commands

```sh
cargo build       # Build
cargo run -- --help                                  # CLI help
cargo run -- run --mode free "add a README section"  # One prompt
cargo test        # Test
cargo fmt         # Format
cargo clippy      # Lint
```
