# alpha-harness

alpha-harness is a coding agent for the desktop and terminal. It works in a
local repository and saves each conversation as a session you can resume later.

## Install

Rust 1.99 or newer is required. Linux desktop builds also need the system
packages listed in [CONTRIBUTING.md](CONTRIBUTING.md#build-from-source).

```sh
cargo install --path apps/alpha-harness
```

For a local build, run `cargo build --release`. The binary is
`target/release/alpha`.

## Start

```sh
alpha desktop
```

Choose a provider in the app and add its API key. The menu in the top-left
corner switches workspaces and sessions. Other open workspaces keep running
when you switch away.

To use the terminal instead:

```sh
alpha tui
```

For a single prompt:

```sh
alpha run "review the error handling in src"
```

## Commands

| Command | Purpose |
| --- | --- |
| `alpha desktop` | Open the desktop app |
| `alpha tui` | Open the full-screen terminal interface |
| `alpha chat` | Chat in the current terminal |
| `alpha run [OPTIONS] [PROMPT]...` | Run one prompt and exit |
| `alpha sessions` | List saved sessions |

Interactive commands use the current directory as the workspace.

### `alpha run`

| Option | Purpose |
| --- | --- |
| `--workspace <PATH>` | Run in another workspace |
| `--mode <MODE>` | Use `read`, `auto`, `reviewed`, or `free` |
| `--provider <ID>` | Select a provider instance or provider kind |
| `--model <MODEL>` | Select a model |
| `--resume <SESSION_ID>` | Continue an existing session |

```sh
alpha run --mode free "fix the failing tests"
alpha run --resume <SESSION_ID> "continue"
```

## Images

In the desktop app, attach an image from the composer, paste it from the
clipboard, or drop a file on the window. In the terminal, use `/image PATH`.

The model can also call `view_image` to inspect a PNG, JPEG, GIF, or WebP file
in the workspace.

## Permissions

`auto` is the default mode.

| Mode | Access |
| --- | --- |
| `read` | Read-only |
| `auto` | Sandboxed work; other access needs approval |
| `reviewed` | Sandboxed work with automatic review for broader access |
| `free` | Full local access without prompts |

Approval prompts can deny a request, allow it once, or allow it for the current
session.

## Files

Provider settings are stored in `~/.config/alpha-harness/config.toml`.
Sessions are stored in `~/.local/state/alpha-harness/sessions`. Both paths
respect the relevant XDG variables. [docs/usage.md](docs/usage.md) lists the
lookup order.

API keys are kept in the system credential store.

## Documentation

- [Usage](docs/usage.md): commands, shortcuts, images, sandbox behavior, files
- [Architecture](docs/architecture.md): modules, core, sessions, and frontends
- [Composition](docs/composition-decoupling.md): trimming and replacing crates
- [Contributing](CONTRIBUTING.md): build, checks, and commits

## License

MIT. See [LICENSE](LICENSE).
