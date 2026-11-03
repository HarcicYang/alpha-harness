# alpha-harness architecture

alpha-harness is a Cargo-composed Rust coding agent with desktop and terminal
frontends. The `alpha` command starts a local core process. The core owns
sessions, providers, tools, permissions, prompts, compaction, and the agent
loop. Frontends communicate with the core through a typed JSON-RPC protocol.

The append-only session log is the durable source of truth. Model history,
restart recovery, file changes, tool state, and frontend projections are all
rebuilt from session events.

## Repository boundaries

| Path | Responsibility |
| --- | --- |
| `apps/alpha-harness` | Main binary, command shell, and full composition |
| `apps/alpha-min` | Trimmed headless composition used to verify decoupling |
| `crates/alpha-kernel` | Component runtime, scopes, services, events, and tasks |
| `crates/alpha-schema` | Shared DTOs, protocol payloads, and durable events |
| `crates/alpha-protocol` | JSON-RPC 2.0 and newline-delimited framing |
| `crates/alpha-core` | Core server, location setup, actors, and RPC methods |
| `crates/alpha-front-*` | Core client, supervisor, shared runtime, TUI, and desktop |
| `crates/alpha-*-api` | Contracts between implementations and consumers |
| `crates/alpha-api-*` | Provider wire-protocol adapters |
| `crates/alpha-module-*` | Replaceable agent behavior |

Repository documentation and code comments use English. The repository does not
use JavaScript, TypeScript, Node.js, or a JavaScript build tool.

## Composition

The composition root selects modules through direct Cargo dependencies. A
module declares its key, scope, requirements, provisions, and capabilities in
`[package.metadata.alpha]`.

`alpha-build` reads the resolved dependency graph during the build. It checks
for missing requirements, duplicate singleton providers, invalid front
declarations, metadata errors, and registry collisions. It writes a generated
module table and front table into `OUT_DIR`; it does not edit source files.

Two crates cannot provide the same singleton capability in one composition.
Replacing a capability means removing one implementation dependency and adding
another. See [composition-decoupling.md](composition-decoupling.md).

## Process model

| Command | Behavior |
| --- | --- |
| `alpha run` | Starts a core for one prompt, streams output, then exits |
| `alpha chat` | Starts a core and uses the line-oriented terminal frontend |
| `alpha tui` | Starts a core and uses the alternate-screen terminal frontend |
| `alpha desktop` | Starts a primary core and opens the GPUI window |
| `alpha sessions` | Starts a core to list stored sessions |

`alpha __core --stdio` is the hidden core entry point. A frontend supervisor
starts this child process, performs the handshake, and manages its lifetime.
The default startup policy allows three retries after the first attempt. After
a crash, the core reopens durable sessions but does not replay an unfinished
turn.

Each terminal process owns its core. The desktop keeps one runtime and core per
opened workspace. One workspace is primary, and every other workspace remains
active in the background. Switching the primary workspace does not stop the
other cores. A session writer lease prevents two processes from writing the
same session.

## Kernel

The kernel provides the runtime used by modules. It does not contain agent
behavior.

The runtime supplies typed contexts, service lookup through parent scopes,
singleton services, named registries, reversible registrations, ordered event
dispatch, and scope-bound task cancellation. Runtime scopes are:

| Scope | Contents |
| --- | --- |
| Global | Provider registries, credentials, and global configuration stores |
| Location | Workspace filesystem, process execution, prompt, compaction, and sessions |
| Agent | One session actor, route overrides, mode, tools, and steer state |
| Call | Cancellation and output state for one active call |

Locations are cached by workspace root and fixed working directory. Idle
locations may be released. Session actors are managed separately and may be
evicted while idle.

## Protocol

The core protocol is JSON-RPC 2.0 over stdio. Each message occupies one line.
Protocol output on stdout contains messages only; diagnostics use stderr.

`alpha-schema` defines requests, notifications, live events, durable events,
provider records, and interaction payloads. Frontends consume typed schema
values. They do not parse protocol JSON into application state.

The protocol covers session creation, resume, attach, event replay, prompts,
cancellation, steering, permission decisions, providers, models, routes,
capabilities, and core information.

Durable events have monotonically increasing sequence numbers. Live events
describe in-process progress and may be dropped. Any state visible to the model
must be reconstructible from durable events. The current event schema is
version 8, and the session header schema is version 1.

## Session storage

The default session root is:

```text
~/.local/state/alpha-harness/sessions
```

`ALPHA_HARNESS_STATE_DIR` and `XDG_STATE_HOME` can move it. Each session
directory contains:

```text
header.json
session.vN.jsonl
session.lock
```

`header.json` contains the session ID, creation time, initial mode, workspace,
provider, and model. The JSONL file contains event envelopes. The writer lease
allows one process to append.

Read-open migrates supported older events in memory without changing the source
file. Repair and migration write a successor generation beside the original.
Recovery drops an incomplete final line, closes an interrupted turn, and
settles every tool call the turn announced. Calls that were recorded without a
result and calls that were only announced both receive an interrupted result,
so the next request sees a complete history. A cancelled turn is settled in the
same way before the actor accepts new work.

The front runtime keeps a chat as an in-memory draft until the first prompt.
Submitting that prompt creates a durable session. Session listings hide
sessions that never recorded an event, so an interrupted startup does not leave
a blank entry in the picker.

## Agent loop

`alpha-module-loop` provides the default agent driver. A turn proceeds until
the model returns no tool calls, the user cancels it, or an operational error
ends it. There is no model-step or tool-call count limit.

A turn opens before any preparation work: the loop appends `turn_started` and
the user message first, then prepares context. Submitting a prompt is therefore
visible and steerable while preparation runs, and a cancelled turn never ends
without a start.

For each model step, the loop:

1. Injects queued steers at a safe boundary.
2. Runs the dynamic context hooks.
3. Appends a durable `request_header` with the context window and output budget
   the request was shaped against.
4. Streams text, thinking blocks, provider tool results, and local tool calls.
5. Appends the provider-reported usage as a `context_usage` event.
6. Appends the canonical assistant message.
7. Executes local tool calls in response order.
8. Continues with the updated durable history.

Expected tool failures are returned to the model. Operational tool failures end
the turn. Cancellation drops the running driver, records an interrupted result
for every call the assistant announced, closes the turn, and unwinds spawned
tasks and listeners through their owning scope.

## Providers and models

Provider kinds own authentication, configuration, model catalogs, compatibility
options, and provider-specific session headers. API adapters own one wire
protocol and its request conversion, stream parser, tool-call accumulation,
usage mapping, and errors.

The full composition includes OpenAI Chat Completions, OpenAI Responses,
Anthropic Messages, DeepSeek, and OpenCode Go. OpenAI-compatible provider kinds
can select a model-specific adapter from their catalog.

Provider and adapter registries are global services. Provider modules declare
the adapter keys they can use. API adapter crates do not depend on provider
modules.

The core can read provider model catalogs when the configured credential allows
it. The bundled catalog remains available offline. Model overrides can change
the display name, adapter, context window, output limit, reasoning support,
provider tools, and default model.

Each session has an effective provider, model, and reasoning level. Route
changes append durable events and apply to the next model step. Chat requests
include the session ID, which adapters may send as a provider-specific routing
header.

## Tools

The default local tools are `read`, `grep`, `write`, `edit`, `shell`, and
`view_image`.

Tools declare JSON Schema input, model-visible content, an optional structured
output, optional metadata, and model-visible images. Before execution, the tool
registry prepares dynamic descriptions and schemas. The request header stores
the tool snapshot advertised to the model.

Tool execution records a durable call before running it. Allowed tools receive
their declared filesystem, process, or HTTP seams. Output can be streamed to
the frontend while a call is active. The final result records completed or
failed status plus any returned images.

`view_image` reads PNG, JPEG, GIF, and WebP files through the filesystem seam
and returns model-visible image content.

`shell` discovers supported local shells, streams output, and can leave a
command running after the current tool call returns. The model can query the
job by `job_id`, choose a per-call wait with `wait_ms`, and read output after a
byte offset with `since_bytes`. The shell tool does not apply a wall-clock
timeout to commands. A foreground command runs in its own process group, so
interrupting the turn kills the command together with its descendants.

## Permissions

| Mode | Read | Workspace write | Outside workspace | Network |
| --- | --- | --- | --- | --- |
| `read` | Allowed | Denied | Denied | Denied |
| `auto` | Allowed | Sandbox rules | Approval required | Denied by default |
| `reviewed` | Allowed | Sandbox rules and review | Review and approval rules | Denied by default |
| `free` | Allowed | Allowed | Allowed | Allowed |

Approval choices are deny, allow once, and allow for the session. Session grants
are durable and end with the session. Static policy grants are configuration,
not session state.

Deny rules take precedence. Protected paths and destructive actions remain
guarded in `auto` and `reviewed`. In `reviewed`, a classifier can pass a
request, send it to the automatic reviewer, or deny it. Reviewer failure closes
the request.

Agents cannot change their own permission mode. Mode changes and permission
decisions are durable events produced through frontend actions.

## Credentials and configuration

Provider secrets use the platform credential store:

| Platform | Backend |
| --- | --- |
| Linux | Secret Service |
| macOS | Keychain |
| Windows | Credential Manager |

Tests use an in-memory store. Provider configuration stores an opaque credential
reference. Frontend responses do not include secret values. Removing a provider
instance also asks the credential backend to delete its secret. There is no
plaintext fallback.

Provider configuration defaults to
`$XDG_CONFIG_HOME/alpha-harness/config.toml`, or
`~/.config/alpha-harness/config.toml` when `XDG_CONFIG_HOME` is unset. Writes
replace the file atomically and update the in-memory registry after the write
succeeds.

The configuration stack orders sources from lowest to highest priority:

1. Module defaults
2. User configuration
3. Project configuration
4. Environment variables
5. Command-line options

Resolved values retain source provenance. Unknown configuration keys are
rejected.

## Prompt and compaction

The prompt module assembles a deterministic system prompt from ordered sections.
The section contract lives in `alpha-prompt-api`, and any crate contributes
through `section_registry` or `context_registry` as a reversible effect of the
context, so registration order between modules does not matter:

- the prompt module owns the agent identity, runtime facts, mandatory rules,
  output contract, stop conditions, and project instructions;
- the tools module owns tool-use policy;
- the permission module owns the permission presets;
- the agent host owns the session-memory pointer.

A section belongs to the crate that owns the fact it describes. Tools remain the
canonical source for tool descriptions and schemas. The host records the
assembled prompt as `system_prompt_set` before the first turn, so every frontend
gets the same prompt and resumed sessions reuse it.

The session-memory section names the session root of the runtime and tells the
model that earlier sessions of this workspace are its long-term memory: it can
search those logs for decisions and preferences that are not in context instead
of guessing.

Project instructions come from `AGENTS.md` and `.alpha/AGENTS.md` between the
workspace root and fixed working directory. The prompt records source paths and
enforces a project-instruction size limit.

The complete system prompt is stored by a durable `system_prompt_set` event
before the first request. Resume uses that stored prompt. Dynamic context is
stored in `context_injected` events and replaced when its rendered content
changes.

The compaction module runs while the turn is open, before the first model step.
It sizes the context from the active model's context window and the prompt
tokens the provider reported for the previous request, and falls back to a
replaceable token estimator for content the provider has not seen yet. Provider
catalogs are discovered once at startup so the window never depends on a
frontend opening a model picker.

Compaction summarizes a complete older turn prefix, reports per-chunk progress
as live events, and writes a durable `context_compacted` event with the replaced
sequence range. The summary keeps a pointer to the session directory, so the
replaced history stays addressable.

When summarization fails the history cannot be compacted in place. The host then
continues the work in a fresh session: the new log opens with a `context_injected`
note that names the previous session directory and the same prompt runs there.
A session that is itself an unusable handoff does not hand off again; the
failure surfaces as an error instead of rotating without end.

## Front runtime

`alpha-front-runtime` contains frontend state and workflows that do not depend
on a terminal or GUI toolkit. It owns session operations, prompt submission,
cancellation, steering, permission decisions, provider operations, transcript
reduction, restart recovery, slash commands, and interaction timeouts.

Provider reads return metadata and credential status but never secrets. The
runtime retains only opaque references.

Context statistics come from the provider: every adapter maps the token usage of
the response it streamed into one usage chunk, and the driver records it as a
durable `context_usage` event. Input tokens are the complete prompt, including
tokens the provider served from its cache, so the cache hit rate is the cached
share of that prompt. The context panel measures the reported prompt, the
completion that joined the history afterwards, and the output budget reserved by
the latest request against the context window that request was shaped against.
It marks the numbers as estimates whenever no usage has been reported yet.

`FrontRuntimeManager` owns one runtime per desktop workspace. Polling reduces
active state changes and background notices. Notices cover background turn
completion, input requests, restarts, and stopped cores.

## Terminal frontend

`alpha-front-tui` owns terminal setup, layout, key and mouse mapping, selection,
clipboard integration, and rendering. It delegates session, turn, provider, and
permission actions to `alpha-front-runtime`.

`chat` stays in the current terminal. `tui` uses the alternate screen. Both
render structured assistant content, tool state, queued steers, permission
prompts, and shared management overlays. Ctrl-C and `/quit` cancel the running
turn and exit after the core has settled it; a second Ctrl-C exits immediately.

## Desktop frontend

`alpha-front-desktop` owns the GPUI Kit window, layout, input mapping,
selection, clipboard operations, scrolling, and presentation animation. Shared
workflow state remains in `alpha-front-runtime`.

| Module | Responsibility |
| --- | --- |
| `shell` | Ambient header, transcript frame, composer placement, and top-level overlays |
| `composer` | Composer editing, pending images, and route or mode popovers |
| `transcript` | User, assistant, thinking, and error rows plus transcript list orchestration |
| `transcript::tools` | Tool group summary, expansion, output, and per-call scroll state |
| `permission` | Inline authorization cards |
| `sessions` | Session search, list, preview, resume, and deletion |
| `providers` | Provider kind and instance management |
| `provider_content` | Provider forms, credentials, validation, and actions |
| `context_panel` | Usage, changes, activity, and capabilities |
| `model_picker` | Model search and selection |
| `command_palette` | Global command search and execution |
| `slash_palette` | Composer slash-command completion |
| `change_review` | File change list and diff view |
| `workspace_menu` | Cascading workspace and session selector |
| `scroll` | Scroll handles, list motion, resets, and edge behavior |
| `ui` and `theme` | Shared visual helpers and color tokens |

The transcript projection is cached in an `Arc<Vec<_>>` and invalidated when
runtime state changes. The transcript uses GPUI `ListState`, so only the range
near the viewport is laid out. Markdown uses stable per-row `TextView` state and
`SharedString` content.

Regular panels use GPUI `ScrollHandle`s. The transcript uses list offsets and an
official GPUI scrollbar. Wheel and touchpad input accumulates pending
displacement; animation frames consume it in either direction with a 32 ms
response constant and a 16 ms frame cap. Scrollbar dragging updates the list
directly. A changed external offset cancels pending wheel motion.

The desktop uses GPUI motion primitives for transcript entry, overlays, tool
expansion, streaming rows, and permission cards. Reduced-motion settings
shorten scroll interpolation. Continuous idle animations are avoided so an
open idle window does not keep repainting text.

The workspace selector is a two-panel menu. Selecting a workspace starts or
activates its core and leaves other workspace cores running. Background notices
appear in the status surface.

An empty transcript contains one large dark gray-blue alpha character centered
in the viewport. It has no surrounding circle, explanatory copy, or buttons.

## Verification

Run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The test workspace covers composition validation, protocol routing, session
append and recovery, provider streaming, prompt and compaction behavior,
permissions, process sandboxing, tools, terminal rendering, desktop reduction,
and end-to-end turns.

CI runs workspace checks on Linux, macOS, and Windows. The Linux quality job
runs format, Clippy, and tests with bounded test-link parallelism. The
composition job builds `alpha-min` and verifies that its command table and
dependency tree omit desktop and terminal UI crates.
