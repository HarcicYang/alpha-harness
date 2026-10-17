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
returned to the TUI. A session is created only when the first non-empty message
is sent, so opening the TUI or starting a new draft does not create an empty
stored session.

The chat header shows `provider/model effort | mode`. `Ctrl-L` opens the model
picker, `Shift-Tab` cycles reasoning effort, and `Ctrl-T` opens the effort
picker. The equivalent commands
are `/model`, `/thinking`, and `/effort`. Route changes are recorded in the
session log and restored when the session is resumed. Model selection also
becomes the persisted default for new sessions, and model pickers prefer the
provider's live `/models` catalog over bundled fallback metadata.

The transcript renders assistant text and reasoning without role prefixes. User
messages use a full-width dark-gray band with one row of vertical padding.
The transcript has no frame or title, and the full terminal canvas uses a
near-black blue background for consistent contrast. Dragging over transcript
text highlights the selection, copies it to the terminal host clipboard when
released, and clears the selection.

Messages and slash commands share one multiline composer. `Enter` sends,
`Ctrl-Enter` inserts a line break, and `Tab` accepts the visible command or
argument completion. Arrow keys, `Home`, `End`, `Backspace`, and `Delete`
edit text at the current cursor position.

`Ctrl-R` opens the session manager. It lists stored sessions with an automatic
preview of the first user message, switches sessions with `Enter`, creates a
new session with `n`, and deletes an idle session with `d` after confirmation.

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
