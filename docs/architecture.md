# alpha-harness architecture

alpha-harness is a Rust coding agent. The `alpha` binary exposes the command
line and starts a local core process for interactive and one-shot work. A small
kernel supplies the component runtime. Agent behavior, providers, tools,
sessions, permissions, prompts, and frontends are modules.

The durable session log determines model history, restart recovery, and every
frontend projection. Any value the model can observe is stored as a session
event or reconstructed from stored events.

## Repository boundaries

- `apps/alpha-harness` builds the `alpha` binary and command-line interface.
- `crates/alpha-kernel` contains contexts, services, typed events, effects,
  scopes, and scope-bound tasks.
- `crates/alpha-schema` contains protocol DTOs, public shared types, and durable
  session payloads.
- `crates/alpha-protocol` implements JSON-RPC 2.0 over newline-delimited JSON.
- `crates/alpha-front-*` contains the supervised core client, shared frontend
  runtime, and presentation adapters.
- `crates/alpha-provider-api` and `crates/alpha-api-*` contain provider and API
  adapter contracts.
- `crates/alpha-module-*` contains replaceable behavior modules.
- `apps/alpha-harness` is the composition root: its direct dependencies decide
  which modules, providers, and frontends this build contains.

The workspace is a Rust 2024 Cargo workspace. `rust-toolchain.toml` selects the
stable toolchain, and `Cargo.lock` is committed.

## Process model

The public commands are:

| Command | Process behavior |
| --- | --- |
| `alpha run` | Starts a managed core for one prompt, streams output, then exits |
| `alpha chat` | Starts a managed core and uses the line-oriented terminal front |
| `alpha tui` | Starts a managed core and uses the alternate-screen terminal front |
| `alpha desktop` | Starts a managed core and opens the GPUI window |
| `alpha sessions` | Starts a managed core to list durable sessions |

The hidden command `alpha __core --stdio` implements the core side of the
front protocol. It is an internal debugging entry point.

A front supervisor starts the hidden core child, performs the protocol
handshake, and owns the child lifetime. The default policy allows three retries
after the initial startup attempt. A restarted core reads durable sessions
again. It does not replay an interrupted turn automatically.

Each terminal front uses one core instance. A front does not attach to another
front's core. The desktop manages one core per opened workspace, keeps one
workspace primary, and polls the other cores in the background. A second writer
for an active session receives `session_busy`.

## Kernel

The kernel owns the component runtime and basic terminal and stdio support.
Substantial behavior does not live there.

The runtime provides:

- Typed contexts and service lookup through parent scopes.
- Singleton services and named registries.
- Declared module requirements and provisions.
- Reversible registrations through disposers.
- Five event dispatch modes: `emit`, `waterfall`, `parallel`, `serial`, and
  `bail`.
- Scope-bound task spawning and cancellation.

Runtime scopes form this chain:

| Scope | Contents |
| --- | --- |
| Global | Composition-level services such as provider registries and credential storage |
| Location | Workspace-bound filesystem, process, configuration, prompt, compaction, and session services |
| Agent | One session actor, route override, mode, tool overlay, and steer queue |
| Call | Ephemeral call state and cancellation |

Location contexts are cached by workspace root and fixed cwd. Removing an idle
Location releases its services. Agent actors are hosted separately from the
Location cache and may be evicted while idle.

## Composition

Modules are ordinary Rust crates selected by the app's Cargo dependency graph.
The composition root lists them as direct dependencies; there is no bundle crate
in between. Module features can remove individual tools or providers from a
crate.

Each module declares its key, requirements, provisions, and capabilities in
`[package.metadata.alpha]`. The app build script calls `alpha_build::check()`,
which reads the resolved graph and validates:

- Missing requirements.
- Duplicate singleton providers.
- Invalid composition overrides.
- Static registry name collisions.
- Module metadata and version compatibility.

The build writes `alpha_composition.rs` to `OUT_DIR`: one activation function per
module, the module table ordered by scope and capability dependency, and the front
command table. It does not modify manifests or source files at build time. The
composition root includes that file with `include!` and passes the resulting
`Host` to the shell.

Two modules may not provide the same singleton capability. A replacement is a
dependency swap, not a coexistence, so the build fails and names both providers.

Runtime plugin loading, dynamic-library modules, and WASM hosting are absent
from the architecture. Modules are trusted compiled Rust code.

## Protocol and schema

The front protocol is JSON-RPC 2.0 over stdio with newline-delimited JSON.
Protocol stdout contains messages only. Logs and diagnostics use stderr.

`alpha-schema` defines the shared request, notification, durable event, live
event, provider, session, and interaction payloads. Frontends consume these
typed values. They do not parse raw protocol JSON into application state.

The protocol covers session creation, resume, attach, event replay, prompt
submission, cancellation, steering, permission decisions, provider management,
model discovery, route changes, capabilities, and core information.

### Durable and live events

Durable events are serializable session facts with monotonically increasing
sequence numbers. Live events report in-process progress and may be dropped.
Persistent state must remain reconstructible from durable events.

The current durable event schema is version 8. The session header schema is
version 1. Older supported events are migrated while reading. A session newer
than the current schema is rejected.

## Session storage

The session directory defaults to:

```text
$ALPHA_HARNESS_STATE_DIR/sessions
```

When that base directory is unset, the store uses:

```text
$XDG_STATE_HOME/alpha-harness/sessions
```

Without `XDG_STATE_HOME`, it uses:

```text
~/.local/state/alpha-harness/sessions
```

One session directory contains:

```text
header.json
session.vN.jsonl
session.lock
```

`header.json` stores the header schema, UUIDv7 session id, creation time,
initial permission mode, workspace binding, provider, and model. The JSONL file
contains append-only event envelopes. The lock file contains the writer lease.

The session writer assigns sequence numbers when it appends events. One process
holds the writer lease. A stale lease can be replaced after recovery checks.

Read-open applies supported event migrations in memory and leaves stored files
unchanged. Repair or migration writes a successor generation next to the source
file. Existing generations are not renamed or deleted.

Recovery drops a torn final line, closes an interrupted turn with a durable
repair event, and records unfinished tool calls as interrupted. Model history,
pending steers, route state, capabilities, prompt snapshots, compacted context,
file changes, and activity are rebuilt from events.

## Agent loop

`alpha-module-loop` provides the default `AgentDriver`. The driver is a module
service and can be replaced by composition.

The default loop processes one turn in this order:

1. Inject pending steers at a step boundary.
2. Run the context injector and compactor hooks.
3. Append a durable `request_header` event with provider, model, reasoning
   level, local tools, and provider-native tools.
4. Stream the model response and publish live step, text, thinking, tool, and
   error events.
5. Append the canonical assistant message with structured content blocks.
6. Execute returned tool calls in response order.
7. Continue until the model returns no tool calls or the step limit is reached.

When the step limit is reached, the loop makes one final request without tools
so the model can summarize the work.

Tool calls and results are durable events and are also represented in model
history. A tool failure that is visible to the model settles as `failed`.
Operational failures end the turn as failed and emit an agent error.

Cancellation is scope-bound. Dropping a scope cancels its spawned work and
unwinds its listeners and services.

### Steering

Steers sent while a turn is active enter a bounded queue. The loop injects
queued steers before the next model step. A steer sent outside an active turn
remains pending until a frontend confirms or discards it.

Accepted pending steers start a new turn. Discarded steers are removed from the
model path but remain identifiable in durable events. The frontend can restore
a discarded message to the composer.

## Providers and models

Provider kinds and API adapters are separate layers.

A provider kind owns authentication, provider configuration, model metadata,
compatibility options, and provider-specific request hooks. An API adapter owns
one wire protocol and its message conversion, streaming parser, tool-call
accumulation, usage mapping, and error mapping.

The default composition includes:

- Generic OpenAI Chat Completions.
- Generic OpenAI Responses.
- Generic Anthropic Messages.
- DeepSeek.
- OpenCode Go.

The available protocol adapters are OpenAI Chat Completions, OpenAI Responses,
and Anthropic Messages.

Provider and adapter registrations use global registries. Provider modules
declare required and optional adapter keys. API adapter crates do not depend on
provider modules.

The core reads model catalogs from a provider's `/models` endpoint when the
stored credential allows it. Known model ids use bundled metadata. New ids use
fallback metadata. The bundled catalog remains the offline fallback.

User configuration can override a model display name, adapter, context window,
output limit, reasoning support, reasoning levels, provider tools, and default
model.

Each session has an effective provider, model, and reasoning level. The initial
route is stored in session creation data. Route changes append durable events
and apply to the next model step. Selecting a model also updates the provider
instance default for new sessions.

Provider requests carry the session id. Adapters that need conversation routing
headers receive it, including OpenCode Go's `x-opencode-session`.

Provider instances use dynamic handles inside session actors. Changing an
endpoint, header, adapter, or other provider configuration affects the next
model step without restarting the session.

## Tools

The default local tools are `read`, `grep`, `write`, `edit`, and `shell`.

A tool implements a typed contract with serde and JSON Schema input, typed
output, model-visible content, and optional UI metadata. Expected failures are
returned to the model. Operational failures end the turn. Cancellation does not
create a tool result.

Tool execution uses this pipeline:

1. Freeze the request's tool snapshot in `request_header`.
2. Validate input.
3. Run the tool pre-execute event.
4. Assert permissions for the requested resources and execution boundary.
5. Execute through the declared seams.
6. Run the post-execute event.
7. Bound model-visible output.
8. Append the durable tool result.

Tools do not call `std::fs`, `std::process`, or sockets directly. The filesystem
and process modules provide swappable drivers. The HTTP module provides the
network seam and cassette recording and replay for tests.

`shell` supports the shell kinds available in the active Location. Linux desktop
builds use bubblewrap for sandboxed process execution when it is available.
There is no macOS or Windows sandbox driver in the current tree.

## Permissions

The permission modes are:

| Mode | Read | Workspace write | Outside workspace | Network |
| --- | --- | --- | --- | --- |
| `read` | Allowed | Denied | Denied | Denied |
| `auto` | Allowed | Sandbox rules | Approval required | Denied by default |
| `reviewed` | Allowed | Sandbox rules plus automated review | Review and approval rules | Denied by default |
| `free` | Allowed | Allowed | Allowed | Allowed |

Approval prompts offer deny, allow once, and allow for the session. Session
grants are durable and end with the session. Persistent grants come from policy
configuration.

Static deny rules take precedence. In `read`, `auto`, and `reviewed` modes,
protected paths and destructive operations remain guarded when a broader rule
would otherwise allow them.

In `reviewed` mode, static policy runs first. A classifier can pass a request
directly under the configured rules. Requests that need more scrutiny go to the
automatic reviewer. Reviewer denial is final. Reviewer failure fails closed.

Permission decisions and their sources are durable session events. Agents cannot
change their own permission mode. Mode changes come from a frontend action.

## Credentials

Provider secrets are stored through `alpha-module-credential`. The module uses:

- Secret Service on Linux.
- Keychain on macOS.
- Credential Manager on Windows.

Tests use an in-memory store. Provider instances store only an opaque
credential reference. Frontend responses do not return secret values. Removing
an instance also requests credential deletion. A failed keyring cleanup is
reported while the instance removal still completes.

If the platform store is unavailable, credential operations fail closed. The
repository has no plaintext credential fallback.

## Configuration

The provider configuration path defaults to
`$XDG_CONFIG_HOME/alpha-harness/config.toml`, or
`~/.config/alpha-harness/config.toml` when `XDG_CONFIG_HOME` is unset.

The file stores provider instances, defaults, and model overrides. Writes use
an atomic replacement and update the in-memory registry after the file write
succeeds.

The shared configuration stack supports these layers, from lowest to highest:

1. Module defaults.
2. User configuration.
3. Project configuration.
4. Environment variables.
5. Command-line flags.

Unknown keys are rejected. Resolved values keep provenance.

## Prompt and context

The prompt module builds a deterministic system prompt from ordered sections.
Sections have stable keys and priorities. Tool schemas remain the canonical tool
descriptions.

Project instructions are read through the filesystem seam from `AGENTS.md` and
`.alpha/AGENTS.md` between the workspace root and fixed cwd. The source path and
size limit are retained.

Before the first model request, the complete system prompt is appended as a
durable `system_prompt_set` event. Resume uses that stored text.

Dynamic context messages are appended as durable `context_injected` events.
They are injected before the first user message and again when their rendered
content changes. Compaction can hide an older injection, causing the current
context to be appended again.

## Compaction

The compaction module runs before model steps. It uses a replaceable token
estimator. The active model's context window takes precedence over the fallback
configuration value.

The default trigger counts growth after the system and compacted-history prefix.
It also enforces the hard context limit. A compaction summarizes a complete
older turn prefix and keeps recent messages.

The summary request runs without tools. If summarization fails, the module stores
a bounded deterministic excerpt. `context_compacted` records the summary and the
sequence range it replaces. History rebuilding inserts the summary before later
events.

## Front runtime

`alpha-front-runtime` contains UI-independent frontend state and workflows. It
depends on the protocol client, supervisor, and schema. It does not depend on a
terminal or desktop toolkit.

The runtime owns:

- Session creation, resume, listing, deletion, and active-session selection.
- Prompt submission, cancellation, steering, and interaction responses.
- Model, reasoning, permission-mode, capabilities, and provider operations.
- Transcript reduction.
- Provider instance and credential workflows.
- Core restart recovery.
- Slash-command behavior and interaction timeout state.
- Durable `StateChanged` signals used by polling adapters.

Provider reads return instance metadata and credential status without secrets.
Secrets are consumed by the credential workflow and are not retained in front
state.

A chat starts as an in-memory draft. The runtime calls `session.create` only
when the first non-empty prompt is submitted.

For the desktop adapter, `FrontRuntimeManager` owns a `FrontRuntime` for every
opened workspace. The active workspace determines the runtime reached through
the manager's `Deref` implementation. Switching the active workspace changes
that pointer and leaves other core supervisors running.

The manager polls every runtime. Active state changes redraw the primary
workspace. Background turn completion, interaction requests, core restarts, and
core disconnections become `WorkspaceNotice` values. The desktop surfaces those
notices in its status toast.

## Terminal frontend

`alpha-front-tui` owns terminal lifecycle, layout, input mapping, selection,
clipboard integration, and rendering. It consumes the shared runtime.

`chat` remains in the current terminal. `tui` uses the alternate screen. The
terminal header displays the route and mode. Markdown tables are rendered for
assistant text, and transcript text selection copies through the terminal host
clipboard on release.

The composer supports multiline editing and command completion. The session
manager, provider manager, model picker, reasoning picker, permission decisions,
and text prompts are terminal presentation states over shared runtime actions.

## Desktop frontend

`alpha-front-desktop` owns the GPUI Kit window, layout, pointer and keyboard
mapping, rendering, scrolling, selection, and UI-local animation. Shared session,
turn, provider, command, and timeout behavior stays in `alpha-front-runtime`.

The presentation modules are:

| Module | Responsibility |
| --- | --- |
| `shell` | Ambient header, transcript frame, composer placement, and top-level overlays |
| `composer` | Multiline composer and route or mode popovers |
| `transcript` | User, assistant, thinking, tool, error, and permission content |
| `permission` | Inline interaction decisions |
| `sessions` | Session list, search, preview, and deletion |
| `providers` | Provider kind and instance management |
| `provider_content` | Provider configuration, credentials, validation, and actions |
| `context_panel` | Usage, changes, activity, and capabilities |
| `model_picker` | Model selection and search |
| `command_palette` | Global command search and execution |
| `slash_palette` | Composer command completion |
| `change_review` | File list and diff review |
| `workspace_menu` | Cascading workspace and session selector |
| `scroll` | Scroll handles, wheel displacement, resets, and scrollbar behavior |
| `session_actions` | Session surface actions |
| `provider_actions` | Provider surface actions |
| `ui` and `theme` | Shared presentation helpers and visual tokens |

Every desktop scroll surface owns an explicit GPUI `ScrollHandle`. The visible
scrollbar is an official GPUI scrollbar placed as a sibling overlay outside the
scrolled content. The content reserves a gutter for the bar.

Wheel and touchpad input adds pending vertical displacement instead of assigning
the handle immediately. Each animation frame consumes part of that displacement
using the frame's elapsed time. New wheel events extend the pending distance.
Scrollbar dragging changes the handle directly; the next render detects the
external movement and discards pending wheel displacement.

The animation uses a 32ms response constant and caps one step at 16ms. The
window requests inactive frames every 8ms, and scrollbar drag refresh allows up
to 120 FPS. Presented frames remain limited by the compositor and display
refresh rate.

Desktop markdown views use stable, per-item `TextView` state. The transcript
projection is cached in an `Arc<Vec<...>>`, and text values are `SharedString`
instances. The cache refreshes after runtime state changes. Its key remains
stable when an in-memory draft receives a durable session id.

Modal, popover, transcript, permission, tool, toast, and streaming-indicator
motion runs through GPUI animation primitives. The adapter checks GPUI's
reduced-motion setting for scroll displacement and uses the toolkit's motion
handling for UI transitions.

The workspace selector is a cascading two-panel menu. The first panel lists
workspace roots, core state, and session counts. Hovering or selecting a
workspace opens the second panel with that workspace's stored sessions and a
new-session action. Opening a workspace creates its core without stopping the
previous workspace core.

When no transcript items exist, the desktop renders only a large, dark gray-blue
Greek alpha character in the center of the transcript viewport. The empty state
has no circle, text, or suggestion buttons.

## Crates

| Crate | Responsibility |
| --- | --- |
| `alpha-harness` | CLI and application composition |
| `alpha-kernel` | Component runtime |
| `alpha-schema` | Shared DTOs and durable payloads |
| `alpha-protocol` | JSON-RPC 2.0 and NDJSON routing |
| `alpha-macros` | Module and composition macros |
| `alpha-build` | Cargo graph validation and generated module table |
| `alpha-credential-api` | Credential storage contract |
| `alpha-host-api` | Shell, module, and front contracts |
| `alpha-http-api` | HTTP seam contract |
| `alpha-front-client` | Protocol client |
| `alpha-front-supervisor` | Core child lifecycle |
| `alpha-front-runtime` | Shared frontend state and workflows |
| `alpha-front-tui` | Terminal frontends |
| `alpha-front-desktop` | GPUI desktop frontend |
| `alpha-provider-api` | Provider and adapter contracts |
| `alpha-api-openai-chat` | OpenAI Chat Completions adapter |
| `alpha-api-openai-responses` | OpenAI Responses adapter |
| `alpha-api-anthropic-messages` | Anthropic Messages adapter |
| `alpha-module-config` | Layered TOML configuration |
| `alpha-module-credential` | Platform credential storage |
| `alpha-module-session` | Session log, projections, and migrations |
| `alpha-module-agent-host` | Session actors and event delivery |
| `alpha-module-loop` | Default agent driver |
| `alpha-module-fs` | Filesystem seam and local driver |
| `alpha-module-process` | Process seam, local runner, and Linux bubblewrap sandbox |
| `alpha-module-http` | HTTP seam and cassette record or replay |
| `alpha-module-permission` | Policy, approval, classifier, and reviewer services |
| `alpha-module-tools` | `read`, `grep`, `write`, `edit`, and `shell` |
| `alpha-module-prompt` | System prompt, project instructions, and context injection |
| `alpha-module-compaction` | Token estimation and durable compaction |
| `alpha-module-provider-config` | Provider instance TOML persistence |
| `alpha-module-provider-openai` | OpenAI-compatible provider kind |
| `alpha-module-provider-openai-chat-completions` | Generic Chat Completions provider kind |
| `alpha-module-provider-openai-responses` | Generic Responses provider kind |
| `alpha-module-provider-anthropic-messages` | Generic Anthropic Messages provider kind |
| `alpha-module-provider-deepseek` | DeepSeek provider kind |
| `alpha-module-provider-opencode-go` | OpenCode Go provider kind |

## Verification

The repository checks are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the quality job on Ubuntu with the Linux desktop build dependencies. A
separate matrix runs `cargo check --workspace --all-targets` on Ubuntu, macOS,
and Windows.

The test suite covers:

- Cargo graph validation and generated composition.
- Session append, replay, migration, repair, leases, and deletion.
- Protocol framing, routing, replay, and server-initiated requests.
- Prompt assembly, project instruction discovery, and context injection.
- Compaction thresholds, summaries, and durable replacement ranges.
- Provider adapters, model metadata, streaming, reasoning, and tool calls.
- Tool behavior, process sandboxing, filesystem boundaries, and HTTP cassettes.
- Permission rules, reviewer behavior, and approval outcomes.
- Terminal rendering, composer editing, session selection, and model selection.
- Shared frontend state and desktop presentation reduction.
