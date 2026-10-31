# alpha-harness

alpha-harness is a local coding agent for software projects. It can inspect a
workspace, edit files, run commands, and keep conversations available for later.
You can use it from the desktop app or a terminal.

## Requirements

- Rust 1.99 or newer.
- Cargo.
- On Linux, the desktop packages listed in `.github/workflows/ci.yml`.
- `bubblewrap` for sandboxed shell commands. Commands still run without it when
  the selected permission mode allows that.

## Build

```sh
cargo build --release
```

The binary is written to `target/release/alpha`. The examples below assume
`alpha` is either on your `PATH` or replaced with that path.

## Start the desktop app

```sh
alpha desktop
```

On first use, configure a provider and model from the app. Then choose a
workspace from the button in the top-left corner and describe the task in the
composer.

The workspace menu has two levels. The first lists workspaces, and the second
lists sessions in the selected workspace. You can also open another folder or
start a new session there. Work in other open workspaces continues in the
background, and the desktop app shows a notice when that work needs attention
or finishes.

Useful desktop shortcuts:

| Shortcut | Action |
| --- | --- |
| `Ctrl-N` | Start a new session |
| `Ctrl-R` | Open saved sessions |
| `Ctrl-P` | Open provider settings |
| `Ctrl-L` | Choose a model |
| `Ctrl-T` | Choose reasoning effort |
| `Ctrl-K` | Open the command palette |
| `?` | Show all shortcuts |

On macOS, use `Cmd` instead of `Ctrl`.

In the composer, `Enter` sends a message, `Shift-Enter` inserts a newline, and
`Tab` accepts a visible slash-command suggestion.

## Terminal interfaces

`alpha chat` keeps the conversation in the current terminal. `alpha tui` uses
the alternate screen.

```sh
alpha chat
alpha tui
```

Both interfaces show the current provider, model, reasoning level, and
permission mode. Use `Ctrl-R` to open saved sessions and `Ctrl-P` to change
provider settings.

## Commands

| Command | What it does |
| --- | --- |
| `alpha desktop` | Open the desktop app |
| `alpha tui` | Open the full-screen terminal app |
| `alpha chat` | Chat without switching to the alternate screen |
| `alpha run [OPTIONS] [PROMPT]...` | Send one prompt and exit |
| `alpha sessions` | List saved sessions |

Common `alpha run` options:

| Option | What it does |
| --- | --- |
| `--workspace <PATH>` | Choose the workspace. The current directory is the default. |
| `--mode <MODE>` | Choose `read`, `auto`, `reviewed`, or `free` permissions. |
| `--provider <ID>` | Choose a provider instance or provider type |
| `--model <MODEL>` | Choose a model |
| `--resume <SESSION_ID>` | Continue a saved session |

```sh
alpha run "review the error handling in src"
alpha run --mode free "fix the failing tests"
alpha run --resume 019a7a30-0000-7000-8000-000000000000 "continue"
```

While a prompt runs, assistant text is printed to stdout. Progress, tool calls,
and errors are printed to stderr.

## Configuration

Provider settings are stored in:

```text
~/.config/alpha-harness/config.toml
```

If `XDG_CONFIG_HOME` is set, the file is under that directory instead.

Credentials use the system credential store: Secret Service on Linux, Keychain
on macOS, and Credential Manager on Windows. alpha-harness never writes API keys
to the configuration file.

Saved sessions are stored in:

```text
~/.local/state/alpha-harness/sessions
```

If `XDG_STATE_HOME` is set, the sessions are under that directory instead. You
can override the base directory with `ALPHA_HARNESS_STATE_DIR`.

## Development

Run the repository checks before submitting changes:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the full checks on Linux and checks the workspace on macOS and Windows.
