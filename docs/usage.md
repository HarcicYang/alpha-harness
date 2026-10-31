# alpha-harness usage reference

Reference material for the terminal interfaces, the desktop app, the shell
sandbox, and the files alpha-harness keeps. The [README](../README.md) covers
installation and the first run.

## Terminal interfaces

```sh
alpha chat
alpha tui
```

`alpha chat` keeps the conversation in the current terminal; `alpha tui` uses
the alternate screen. Both show the current provider, model, reasoning level,
and permission mode, and both print the session ID with a `/resume` hint on
exit.

| Key | Action |
| --- | --- |
| `Enter` | Send the message |
| `Ctrl-Enter`, `Ctrl-J` | Insert a newline |
| `Tab` | Accept the visible slash-command completion |
| `Shift-Tab` | Select the previous completion, or cycle reasoning effort when none is visible |
| `Esc` | Dismiss a completion, recall a queued prompt, or cancel the turn |
| `Ctrl-P` | Provider settings |
| `Ctrl-L` | Choose a model |
| `Ctrl-T` | Choose reasoning effort |
| `Ctrl-R` | Open saved sessions |
| `PageUp`, `PageDown`, wheel | Scroll the transcript |
| `Ctrl-C` | Quit |

### Slash commands

Both interfaces accept the same commands, and `Tab` completes them.

| Command | What it does |
| --- | --- |
| `/provider [ID]` | Open the provider manager or switch provider |
| `/model [MODEL] [EFFORT]` | Open the model picker or switch the route |
| `/thinking LEVEL`, `/effort LEVEL` | Set the reasoning effort |
| `/mode MODE` | Set `read`, `auto`, `reviewed`, or `free` |
| `/confirm`, `/discard` | Run or discard pending steers |
| `/capabilities TOOLS` | Set provider-native tools, for example `web_search` |
| `/new` | Start a new session draft |
| `/resume ID` | Resume a session |
| `/sessions` | Open the session manager |
| `/diagnostics` | Show core connection details |
| `/help`, `/quit` | Show help, exit |

## Desktop app

The workspace menu has two levels. The first lists workspaces, and the second
lists sessions in the selected workspace. You can also open another folder or
start a new session there. Work in other open workspaces continues in the
background, and the desktop app shows a notice when that work needs attention
or finishes.

| Shortcut | Action |
| --- | --- |
| `Ctrl-N` | Start a new session |
| `Ctrl-R` | Open saved sessions |
| `Ctrl-P` | Open provider settings |
| `Ctrl-L` | Choose a model |
| `Ctrl-T` | Choose reasoning effort |
| `Ctrl-K` | Open the command palette |
| `?` | Show all shortcuts |
| `Enter` | Send the message |
| `Shift-Enter` | Insert a newline |
| `Tab` | Accept a visible slash-command suggestion |

On macOS, use `Cmd` instead of `Ctrl`.

## Shell sandbox

On Linux, shell commands run inside `bubblewrap`: the workspace is writable,
system paths and the home directory are read-only, `/tmp` is a private tmpfs,
and the network is unshared by default.

- Sandboxing is controlled by module configuration (`sandbox.enabled`,
  `sandbox.network`), not by the permission mode. The default is to probe for a
  working `bwrap` at startup.
- If `bwrap` is missing and `sandbox.enabled` is unset, alpha-harness warns and
  disables the sandbox. Commands then run unsandboxed, so in `auto` and
  `reviewed` every non-read-only command asks for approval first.
- If `sandbox.enabled = true` and `bwrap` is missing, startup fails with an
  error instead.
- Without an approval channel, an unsandboxed request is denied rather than
  allowed.
- The bubblewrap sandbox targets Linux. The user configuration file does not
  currently expose these settings.

## Providers

Provider kinds are modules, so the panel lists exactly the kinds compiled into
your build. The reference composition is listed in
[`architecture.md#providers-and-models`](architecture.md#providers-and-models).

## Files and environment

Provider settings are stored in:

```text
~/.config/alpha-harness/config.toml
```

The path resolves in this order: `$XDG_CONFIG_HOME/alpha-harness/config.toml`,
then `$HOME/.config/alpha-harness/config.toml`, then
`./.config/alpha-harness/config.toml`. The file holds provider instances, the
default instance, and per-instance model overrides.

Saved sessions are stored in:

```text
~/.local/state/alpha-harness/sessions
```

The path resolves in this order: `$ALPHA_HARNESS_STATE_DIR/sessions`,
`$XDG_STATE_HOME/alpha-harness/sessions`, then
`$HOME/.local/state/alpha-harness/sessions`, then `./sessions`.

macOS sets `HOME`, so the defaults above apply. Windows usually does not, which
puts both paths under the current directory; set `HOME` or `XDG_*` there to move
them.

`alpha sessions` prints one tab-separated line per session with the fields `id`,
permission mode, creation time in milliseconds, and workspace root.
