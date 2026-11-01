# alpha-harness usage

This page covers the terminal and desktop apps, images, permissions, shell
commands, and local files. [README.md](../README.md) covers installation and
the first run.

## Commands in a build

The `alpha` binary contains the frontends and provider kinds linked into that
build. Removing a frontend crate removes its command. Removing a provider kind
removes it from the provider list.

## Terminal

```sh
alpha chat
alpha tui
```

`alpha chat` keeps the conversation in the current terminal. `alpha tui` uses
the alternate screen. Both show the provider, model, reasoning level, and
permission mode. The session ID is printed on exit.

| Key | Action |
| --- | --- |
| `Enter` | Send the message |
| `Ctrl-Enter`, `Ctrl-J` | Insert a newline |
| `Tab` | Accept the visible slash-command completion |
| `Shift-Tab` | Previous completion, or reasoning effort when no completion is shown |
| `Esc` | Dismiss a completion, recall a queued prompt, or cancel the turn |
| `Ctrl-P` | Open provider settings |
| `Ctrl-L` | Choose a model |
| `Ctrl-T` | Choose reasoning effort |
| `Ctrl-R` | Open saved sessions |
| `PageUp`, `PageDown`, mouse wheel | Scroll the transcript |
| `Ctrl-C` | Quit |

### Slash commands

`alpha chat` and `alpha tui` accept the same commands.

| Command | Action |
| --- | --- |
| `/provider [ID]` | Open the provider manager or switch provider |
| `/model [MODEL] [EFFORT]` | Open the model picker or switch the route |
| `/thinking LEVEL`, `/effort LEVEL` | Set reasoning effort |
| `/mode MODE` | Set `read`, `auto`, `reviewed`, or `free` |
| `/image PATH` | Attach an image to the next prompt |
| `/confirm`, `/discard` | Run or discard pending steers |
| `/capabilities TOOLS` | Set provider-native tools |
| `/new` | Start a new session draft |
| `/resume ID` | Resume a session |
| `/sessions` | Open the session manager |
| `/diagnostics` | Show core connection details |
| `/help`, `/quit` | Show help or exit |

## Desktop

The top-left menu lists workspaces in its first panel and sessions in its
second panel. You can open another folder or create a session for the selected
workspace there. Switching workspaces changes the primary core. Other workspace
cores stay running, and the app reports background completion, disconnection,
or input requests.

| Shortcut | Action |
| --- | --- |
| `Ctrl-N` | Start a new session |
| `Ctrl-R` | Open saved sessions |
| `Ctrl-P` | Open provider settings |
| `Ctrl-L` | Choose a model |
| `Ctrl-T` | Choose reasoning effort |
| `Ctrl-K` | Open the command palette |
| `Ctrl-V` | Paste an image from the clipboard |
| `?` | Show shortcuts |
| `Enter` | Send the message |
| `Shift-Enter` | Insert a newline |
| `Tab` | Accept a slash-command suggestion |

On macOS, use `Cmd` in place of `Ctrl`.

Wheel, touchpad, and scrollbar movement update the transcript through the GPUI
list and settle smoothly in both directions. Tool output has its own scroll
position.

## Images

In the desktop app, use the attach button, paste an image while the composer is
focused, or drop an image on the window. Pending images appear above the
composer and can be removed before sending. The terminal uses `/image PATH`.

Images are stored with the user message in the session log and encoded for the
selected provider adapter. The model can call `view_image` to inspect a PNG,
JPEG, GIF, or WebP file in the workspace without waiting for an attachment.

## Long-running work

The agent does not impose a model-step or tool-call count limit for a turn. A
turn ends when the model responds without tool calls, the user cancels it, or
an operational error ends it.

Shell commands have no built-in wall-clock timeout. `shell` can return a job ID,
and later calls can wait for the job and read output from a byte offset with
`job_id`, `wait_ms`, and `since_bytes`. A response is capped at 64 KiB and
marks truncated output, while a running job keeps producing output. The desktop
and TUI receive streamed output while the command runs.

## Shell sandbox

On Linux, sandboxed commands run through `bubblewrap`. The workspace is
writable. System paths and the home directory are read-only, `/tmp` is a private
tmpfs, and networking is disabled by default.

| Setting | Behavior |
| --- | --- |
| `sandbox.enabled = true` | Fail startup when `bwrap` is unavailable |
| `sandbox.enabled = false` | Run commands without `bubblewrap` |
| unset | Probe for `bwrap` and warn when it is unavailable |
| `sandbox.network = true` | Keep networking enabled in the sandbox |
| `sandbox.read_only` | Add read-only bind mounts |
| `sandbox.writable` | Add writable bind mounts |

Without an approval channel, an unsandboxed request is denied.

## Providers and output limits

Provider kinds are selected at build time, so the provider panel only lists the
kinds in the binary. The model catalog supplies context windows and model
output limits.

When a model has an output limit, the adapter sends it as `max_tokens` for Chat
Completions and Anthropic Messages, or `max_output_tokens` for Responses. An
explicit matching provider option takes precedence. Models without a catalog
limit send no output-limit field, leaving the provider default in control.

## Files and environment

Provider settings are stored at:

```text
~/.config/alpha-harness/config.toml
```

The path resolves in this order:

1. `$XDG_CONFIG_HOME/alpha-harness/config.toml`
2. `$HOME/.config/alpha-harness/config.toml`

When `HOME` is unset, the fallback is
`./.config/alpha-harness/config.toml`.

The file stores provider instances, the default instance, and model overrides.
API keys are kept in the system credential store.

Sessions are stored at:

```text
~/.local/state/alpha-harness/sessions
```

The state path resolves in this order:

1. `$ALPHA_HARNESS_STATE_DIR/sessions`
2. `$XDG_STATE_HOME/alpha-harness/sessions`
3. `$HOME/.local/state/alpha-harness/sessions`

When `HOME` is unset, the fallback is `./sessions`.

`alpha sessions` prints one tab-separated line per session with the session ID,
permission mode, creation time in milliseconds, and workspace root.
