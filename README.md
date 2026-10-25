# alpha-harness

A lightweight coding agent harness built from a small kernel and replaceable
modules. The design decisions live in
[docs/architecture.md](docs/architecture.md).

## Layout

- `apps/alpha-harness`: the `alpha` binary.
- `crates/`: the kernel, protocol, frontends, and modules.

## Build

```sh
cargo build --release
```

The workspace requires Rust 1.99 or newer.

## Run a prompt

```sh
alpha run --mode free "add a README section"
```

The command accepts these options:

- `--mode read|auto|reviewed|free` sets the permission mode. The default is
  `auto`.
- `--workspace <path>` sets the workspace root. Without it, the current
  directory is used.
- `--provider <ID>` selects a configured provider instance. Otherwise the
  configured default is used.
- `--model <MODEL>` selects a model. Otherwise the provider catalog default is
  used.

Provider credentials are stored in the platform keyring and configured with
`alpha tui`. The harness does not read credentials from environment variables.

Interactive terminals show approval prompts in `auto` mode. Non-interactive
runs fail closed unless you use `--mode free` or add explicit policy rules.
Shell commands run in a bubblewrap sandbox when one is available.

Assistant output goes to stdout as it arrives. Step, tool, and error progress
goes to stderr.

## Interactive frontends

```sh
alpha chat
alpha tui
alpha desktop
```

These commands run a managed core through the shared front runtime. `chat`
stays in the current terminal, `tui` uses the alternate screen, and `desktop`
opens the GPUI Kit window. The runtime discovers provider kinds and stored
provider instances, and supports creating, removing, and validating them,
setting the default, entering credentials, editing the provider configuration
as JSON, and refreshing provider state. The core writes secrets to the platform
keyring and never sends them back to the front.

The front creates a session only after the first non-empty message is sent.
Opening a front or starting a new draft does not create an empty session.

### Terminal frontends

The terminal chat header shows `provider/model effort | mode`. `Ctrl-L` opens
the model picker, `Shift-Tab` cycles the reasoning effort, and `Ctrl-T` opens
the effort picker. The same actions are available as `/model`, `/thinking`, and
`/effort`. Route changes are written to the session log and restored on resume.
Selecting a model also stores it as the default for new sessions. The model
picker uses the provider's live `/models` catalog when available and falls back
to bundled metadata.

Assistant text and reasoning are rendered without role prefixes. User messages
use a full-width dark-gray band with one row of padding above and below. The
transcript has no frame or title, and the terminal canvas uses a near-black
blue background. Dragging across transcript text highlights the selection,
copies it to the terminal host clipboard on release, and clears the selection.

Messages and slash commands share one multiline composer. `Enter` sends,
`Ctrl-Enter` inserts a line break, and `Tab` accepts the visible command or
argument completion. The arrow keys, `Home`, `End`, `Backspace`, and `Delete`
edit text at the cursor.

`Ctrl-R` opens the session manager. It lists stored sessions with a preview of
the first user message. Press `Enter` to switch sessions, `n` to create one, or
`d` to delete an idle session after confirmation.

### Desktop

The desktop shell uses one centered conversation column with an ambient header.
Sessions, context, and commands open only when requested instead of taking
permanent screen space. The composer shows route and permission capsules,
supports path attachment, and follows the shared runtime transcript without
duplicating turn or session behavior.

- `Ctrl-N` starts a draft, `Ctrl-R` opens stored sessions, and `Ctrl-P` opens
  provider management.
- `Ctrl-L` opens the model picker, `Ctrl-T` opens reasoning effort, and
  `Ctrl-K` opens the global command palette.
- `Enter` sends, `Shift-Enter` inserts a line break, and `Tab` completes a slash
  command.
- Permission requests appear in the transcript with deny, once, and session
  decisions. The context panel exposes token usage, file changes, activity,
  and current capabilities; provider-native capabilities can be edited there
  or from the global command palette.

The desktop implementation is split by presentation concern under
`crates/alpha-front-desktop/src`. `shell`, `composer`, `transcript`, session,
provider, context, and permission modules consume typed runtime state. Motion
and color tokens live in the desktop presentation layer; session control,
provider workflows, commands, transcript reduction, and interaction timeouts
remain in `alpha-front-runtime`.

## Prompting and context

- The workspace root and fixed cwd are searched for `AGENTS.md` and
  `.alpha/AGENTS.md`. Their contents are added to the system prompt.
- The complete prompt is stored in the session log and replayed on resume.
- Long histories are compacted into a durable summary before the configured
  context budget runs out. If compaction fails, the original history is left
  unchanged.

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
