# alpha-harness

alpha-harness is a Rust coding agent. The `alpha` binary starts a local core,
opens a shared front runtime, and exposes one-shot, terminal, or GPUI desktop
interfaces. Session state is stored in an append-only event log.

## Requirements

- Rust 1.99 or newer. The active toolchain is declared in `rust-toolchain.toml`.
- Cargo.
- Linux desktop builds need the X11, Wayland, Vulkan, fontconfig, ALSA, and
  xkbcommon development packages listed in `.github/workflows/ci.yml`.
- Sandboxed shell commands use `bubblewrap` when it is available.

## Build

```sh
cargo build --release
```

The resulting binary is `target/release/alpha`.

## Commands

| Command | Purpose |
| --- | --- |
| `alpha run [OPTIONS] [PROMPT]...` | Run one prompt and exit |
| `alpha chat` | Open the terminal chat without the alternate screen |
| `alpha tui` | Open the full-screen terminal interface |
| `alpha desktop` | Open the GPUI desktop window |
| `alpha sessions` | List stored sessions |

`alpha run` accepts these options:

| Option | Effect |
| --- | --- |
| `--mode read\|auto\|reviewed\|free` | Sets the permission mode. The default is `auto`. |
| `--workspace <PATH>` | Sets the workspace root. The current directory is the default. |
| `--provider <ID>` | Selects a provider instance or provider kind. |
| `--model <MODEL>` | Selects a model from the active provider catalog. |
| `--resume <SESSION_ID>` | Resumes an existing session instead of creating a new one. |

```sh
alpha run --mode free "add a README section"
alpha run --resume 019a7a30-0000-7000-8000-000000000000 "continue"
```

Assistant text is written to stdout as it streams. Progress, tool events, and
errors are written to stderr.

## Terminal interface

`alpha chat` stays in the current terminal. `alpha tui` uses the alternate
screen. Both use the shared front runtime and managed core process.

The prompt is a multiline composer with slash-command completion:

- `Enter` sends the prompt.
- `Ctrl-Enter` or `Ctrl-J` inserts a line break.
- `Tab` accepts the visible slash-command completion.
- `Esc` closes the active picker or returns focus to the composer.

The header shows the current provider, model, reasoning level, and permission
mode.

| Shortcut | Action |
| --- | --- |
| `Ctrl-L` | Open the model picker |
| `Shift-Tab` | Cycle reasoning effort |
| `Ctrl-T` | Open the effort picker |
| `Ctrl-P` | Open provider settings |
| `Ctrl-R` | Open the session manager |

The slash commands `/model`, `/effort`, and `/thinking` expose the same route
controls. Route changes are appended to the session log and restored on resume.

The session manager shows the first user message for each session. `Enter`
switches sessions, `n` creates a draft, and `d` asks for confirmation before
deleting an idle session.

## Desktop interface

`alpha desktop` uses a centered conversation column and an ambient workspace
header. Sessions, context, commands, model selection, reasoning effort,
provider settings, and permission mode open in overlays or popovers.

`Ctrl` shortcuts use `Cmd` on macOS.

| Shortcut | Action |
| --- | --- |
| `Ctrl-N` | Start a new draft |
| `Ctrl-R` | Open the session manager |
| `Ctrl-P` | Open provider settings |
| `Ctrl-L` | Open the model picker |
| `Ctrl-T` | Open the reasoning-effort picker |
| `Ctrl-K` | Open the global command palette |
| `?` | Open the shortcut reference |

`Enter` sends, `Shift-Enter` inserts a line break, and `Tab` accepts a visible
slash-command completion. Permission requests appear in the transcript with
deny, once, and session choices.

The context panel shows token usage, changed files, recent activity, and the
provider capabilities for the current session. Provider instances and
capabilities can also be changed from keyboard commands.

Desktop scroll areas share an explicit GPUI `ScrollHandle` between the content
viewport and a visible scrollbar. Wheel and touchpad deltas accumulate as
pending displacement and are consumed on animation frames. Scrollbar dragging
moves the handle directly and cancels pending displacement. The transcript
keeps a cached projection and stable Markdown state so a draft that becomes a
stored session does not rebuild the first turn.

## Configuration and credentials

The user configuration file defaults to:

```text
$XDG_CONFIG_HOME/alpha-harness/config.toml
```

Without `XDG_CONFIG_HOME`, the path is:

```text
~/.config/alpha-harness/config.toml
```

Provider instances, the default provider, default models, and model overrides
are stored there. Credentials are written through the platform credential
store:

- Secret Service on Linux.
- Keychain on macOS.
- Credential Manager on Windows.

Provider state keeps an opaque credential reference. Credential values are not
returned to a frontend or written to the TOML file. If the platform credential
store is unavailable, credential operations fail instead of falling back to
plaintext storage.

## Session storage

The session directory defaults to:

```text
$XDG_STATE_HOME/alpha-harness/sessions
```

Without `XDG_STATE_HOME`, the path is:

```text
~/.local/state/alpha-harness/sessions
```

`ALPHA_HARNESS_STATE_DIR` can replace the base state directory.

Each session directory contains a header, one or more append-only JSONL event
generations, and a writer lease. Opening a session for reading can apply
in-memory migrations. Repair or migration creates a successor generation and
leaves earlier files in place.

## Development

Run the repository checks before committing:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs those checks on Ubuntu and runs workspace checks on Ubuntu, macOS, and
Windows. The test suite covers composition validation, session recovery,
permission decisions, provider adapters, protocol framing, prompt assembly,
compaction, terminal behavior, and frontend state.
