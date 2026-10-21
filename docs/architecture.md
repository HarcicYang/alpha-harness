# alpha-harness Architecture

Status: frozen for M1. This is the authoritative architecture document.

## 1. Vision

alpha-harness is a lightweight coding agent harness. It uses a small kernel
and replaceable modules, with the same approach to composition that NixOS uses.

Two ideas guide the design:

1. Keep the kernel small and make every substantial feature opt-in.
2. Everything is a module. Every capability is atomic, self-contained, and
   independently replaceable.

## 2. Hard principles

- P1: Substantive behavior lives in modules. The kernel contains only the
  component runtime and basic stdio. User-visible behavior, including the
  agent loop, providers, tools, sessions, permissions, and UI, belongs in a
  module.
- P2: The repository does not use the JavaScript or TypeScript ecosystem. No
  Node.js, npm, pnpm, or JS tooling may enter the build chain, documentation
  tooling, shipped modules, or runtime dependencies. Language-neutral
  protocols such as JSON-RPC and MCP remain allowed.

## 3. Architecture decisions

### D1: Kernel scope

The kernel provides the component runtime. It manages contexts, service
registries, declared dependencies (reactive coeffects), typed events,
reversible effects, scopes, lifecycle, and scope-bound task spawning. Its only
IO is terminal and stdio.

### D2: Agent loop is a module

The default ReAct loop is a module, not part of the kernel. Consumers depend on
the `AgentDriver` trait. The default `AgentLoop` registers under the same
service key, which lets a composition replace it with another driver. Each
turn receives its agent-scoped `Ctx`, so progress is emitted as typed live
events instead of through a loop-specific observer callback.

### D3: Compile-time composition

Modules are Rust crates and composition happens at compile time. M1 has no
runtime plugin loader, dynamic library ABI, or WASM host.

### D4: Module granularity

A capability is the unit of replacement. One module may provide several
capabilities, such as tools, adapters, or policies, as long as each has a
stable key and can be enabled, disabled, or replaced on its own. A module
boundary is not a replacement boundary. Bundles only wire dependencies
together and contain no behavior.

### D5: Singleton and registry capabilities

- Singleton capabilities have exactly one provider. A second provider is a
  build error unless the composition declares an override.
- Registry capabilities, such as tools, adapters, and commands, accept
  multiple providers keyed by name. Static name collisions fail the build.
  Dynamic sources use namespaces such as `mcp__<server>__<tool>`.

### D6: Composition source is the Cargo dependency graph

The app declares modules as ordinary dependencies in `Cargo.toml`. Each module
crate lists its capabilities in `[package.metadata.alpha]`. During the build,
`build.rs` calls `alpha_build::check()`, which:

1. reads the resolved dependency graph and enabled features,
2. validates the composition (missing requirements, singleton conflicts,
   invalid overrides, static registry name collisions, metadata version),
3. generates an explicit module table into `OUT_DIR`.

`main.rs` includes the generated table through `alpha_kernel::bootstrap!()`.
Build-time tooling writes only to `OUT_DIR` and reports problems as
diagnostics. It does not edit manifests or source files, so any suggested fix
must be applied by the user. If an `alpha fix` helper is added later, it must
default to a dry run and require confirmation before making changes.

### D7: Options are runtime configuration

Composition and module presence are fixed at compile time. Tunable values such
as model names, timeouts, and limits live in TOML and follow this precedence:

module defaults < user config < project config < environment < CLI flags.

Modules declare a serde schema and metadata for their options, including
documentation, whether a value is secret, whether a restart is required, and
its scope. The CLI, TUI, and generated documentation use that metadata.
Unknown keys are rejected. Effective values keep their provenance so
`alpha config show` can report which layer supplied each value.

### D8: The session log is the single source of truth

If the model can observe something, it must be in the log. Everything the
model sees has to be reconstructible from durable, append-only session events.
The UI, statistics, resume, fork, compaction, and telemetry are projections
over that log. New model-visible behavior needs new event types and versioned
migrations. Compaction appends events instead of rewriting history.

### D9: IO boundaries

The kernel provides stdio only. Filesystem, process, and network access are
seam modules with swappable drivers:

- filesystem: local driver first; remote-workspace, memory, unavailable later
- process: local driver first; remote and unavailable later
- network: local HTTP driver first; record/replay driver for tests

Modules must not call `std::fs`, `std::process`, or sockets directly. This is
both a contract and a CI check. Tools assert permissions explicitly, and the
tool registry does not wrap calls in implicit permission checks. Trusted
built-ins capture the services they need; application tools receive only the
capabilities they require.

### D10: Scopes

Runtime scopes form a chain: Global, Location, Agent, and Call.

- Global contains composition-level singletons such as telemetry and the
  credential store.
- Location contains the project service graph: filesystem root, permission
  policy, execution world, project config, watchers, and session storage.
- Agent contains one session or agent: loop, model route, session state, and
  tool overlay.
- Call contains cancellation tokens and other ephemeral effects.

Location graphs are created when needed and can be reclaimed while idle.

### D11: Workspace determines cwd

A workspace defines where execution happens, either at a local root or on a
remote target. Nothing below the workspace may choose its own cwd:

- The Location cwd is fixed when the Location is created, either at the
  workspace root or at a subdirectory.
- Processes, filesystem, git, search, and watchers all derive paths from the
  Location.
- The harness process's own OS cwd is never consulted. CI lint bans
  `current_dir` and `set_current_dir` outside the workspace module.
- Changing cwd requires explicitly rebinding to a new Location, recorded as a
  durable event.
- Agents, tools, and individual calls have no cwd override parameter.

### D12: Delivery form

Headless and SDK first:

- `alpha run`: one-shot and streaming stdout, for scripts and CI.
- `alpha chat`: interactive managed-core chat front.
- `alpha tui`: full-screen terminal profile backed by a managed core.
- `alpha __core --stdio`: hidden core process managed by a front.
- Rust SDK embeds the core crates directly.

`chat` and `tui` share the same UI-independent front runtime and differ only in
their terminal profile. A daemon can be added later because the protocol and
transport are separate.

### D13: External protocol

The protocol is JSON-RPC 2.0 over stdio with newline-delimited JSON framing.
stdout carries protocol messages only, and logs go to stderr. The
`alpha-schema` crate owns public shapes, durable payloads, provider metadata,
and typed front protocol messages such as `SessionEventNotification`. The
first carrier is the private stdio pipe between a front supervisor and its
core child. Other carriers can be added without changing the protocol.

Methods include session lifecycle (`session.create`, `session.list`,
`session.resume`, `session.attach`, `session.events`), message submission,
cancellation, and approval requests (`permission.request` as a server-initiated
request).

### D14: Events

Events fall into two classes:

- Durable events record session facts. They are serde-serializable, versioned,
  and carry a monotonically increasing `seq`. Resume, replay, projections, and
  telemetry use them.
- Live events support process-local observation and interception. They may
  contain values that cannot be serialized, such as handles or cancellation
  tokens, and the runtime may drop them. Any state worth keeping must be
  reconstructible from durable events.

Each event uses one of five dispatch modes: `emit`, `waterfall`, `parallel`,
`serial`, or `bail`. The mode is part of the event's type contract and is
enforced with sealed marker traits. Event domains are namespaced as
`session/*`, `agent/*`, `tools/*`, `fs/*`, and `perm/*`. A scope owns listener
registrations, and removing the scope reverses them.

### D15: Concurrency and writing

- Each agent has one driver task with a bounded mailbox. Agent state is
  mutated only inside that task.
- Each session has one writer. The driver owns the write handle, and the
  session module flushes batches through an internal writer task. Sequence
  numbers are assigned when an event is appended.
- Tool calls run with bounded parallelism (default 4). Tools declare
  `parallel-safe` or are exclusive by default; exclusive calls act as ordering
  barriers. Durable results commit in call order; live progress is immediate.
- Cancellation uses a token tree with scope, turn, step, and call levels.
- Background work must use `ctx.spawn` (scope-bound, cancelled and joined on
  scope close). Raw `tokio::spawn` is banned by lint.

### D16: Repository layout

The repository is a Cargo workspace. The app package `alpha-harness` lives in
`apps/alpha-harness` and produces the `alpha` binary. Modules and libraries
live under `crates/`, and the toolchain is pinned in `rust-toolchain.toml`.

### D17: Module distribution

Modules are ordinary crates that can be distributed through crates.io, git, or
path dependencies. The `alpha-module` keyword is used for discovery. The
kernel API follows semver, and during 0.x a breaking change bumps the minor
version. The app commits `Cargo.lock`. Third-party modules are trusted,
compiled code; capability discipline comes from contracts and linting, not
from a sandbox. CI runs supply-chain checks with cargo-deny and cargo-audit.

### D18: Tools

Built-in and third-party tools use the same contract:

- Typed input via serde plus schemars; typed machine output; non-empty
  model-visible content; optional UI metadata (three-value result).
- Errors are classified: `ToolFailure` (expected, model-visible),
  operational errors, and cancellation (never a tool result).
- Execution follows one pipeline: a per-request tool snapshot is frozen in
  `request/header`, input validation, `tools/pre-execute` waterfall, explicit
  permission assertion, execution, `tools/post-execute`, normalization and
  bounding (head plus tail truncation), durable commit in call order.
- Registrations are Location-scoped with Agent overlays. Static names are
  build-checked; dynamic names are namespaced.
- Tool-level pruning uses Cargo features inside a module crate.

M1 tools: `read`, `grep`, `write`, `edit`, `shell`.

The tool input and output types:

```rust
pub struct ShellInput {
    pub shell: Option<ShellKind>, // defaults to the Location default shell
    pub command: String,
    pub timeout_ms: Option<u64>,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub struct ShellOutput {
    pub shell: ShellKind,
    pub exit_code: i32,
    pub duration_ms: u64,
    pub truncated: bool,
}
```

`shell` replaces a separate bash tool. The process seam detects available
shells per Location, so a remote workspace reports its own list. The tool
description and schema list the available shells and the default. If a shell
is unavailable or unknown, the model receives a failure that lists the valid
options. Shell kinds are registry entries (`bash`, `zsh`, `fish`, `sh`,
`pwsh`, `powershell`, `cmd`, and third-party kinds) with per-kind argv
templates. The default shell is resolved from module config, then `$SHELL`,
then a platform fallback. Shell calls are exclusive, assert `shell.exec`,
inherit the parent environment with an optional denylist, cancel by killing
the process group, and stream stdout and stderr as tagged live progress.

### D19: Permission modes and sandbox

Four user-facing modes:

| Mode | Sandbox | Boundary behavior |
| --- | --- | --- |
| `read` | read-only, network off, no execution | mutate requests denied, no prompts |
| `auto` (default) | workspace-write plus temp dirs, network off, protected paths read-only | sandbox-contained actions run automatically; requests outside the sandbox ask for synchronous approval |
| `reviewed` | same as `auto` | two-stage review chain, high risk prompts the user |
| `free` | full access, network on | no prompts |

Filesystem and process tools choose their execution boundary from the
effective mode on every call. `auto` and `reviewed` use workspace-rooted
filesystem access and sandboxed process runners. `free` uses full-access local
filesystem and process runners. A mode change therefore takes effect without
rebuilding the session runtime.

M1 ships `read`, `auto`, `reviewed`, and `free`.

Approval prompts offer three choices: deny, allow once, and allow for this
session. There is no "always allow" option. Session grants are durable events,
survive resume, and expire with the session. Persistent grants exist only when
the user edits a policy file.

Never auto-approved: critical-path deletions, credential files, policy and
sandbox configuration, push and publish/deploy operations, and cross-session
messaging.

The `reviewed` mode makes decisions in this order:

1. Static rules; deny wins; protected classes are intercepted.
2. The intent classifier decides whether the request can proceed directly. By
   default, sandbox-contained requests are allowed and everything else goes to
   review.
3. After classifier handoff, the automatic approval reviewer makes the final
   decision. An `allow` proceeds; a `deny` is final and the action does not
   run.
4. The default reviewer allows local read-only actions and denies other
   unsandboxed requests. Custom classifier and reviewer providers can be
   mounted through the permission module; reviewer failures fail closed by
   returning `deny`.
5. Verdicts and their decision sources are recorded as durable events.

The classifier and reviewer are replaceable service seams. A model-backed
implementation can classify intent and assess risk, while the built-in
providers use deterministic, conservative rules. Reviewer requests contain the
action, resources, model-provided reason, and sandbox-boundary status. Raw tool
output and repository content are not included.

The sandbox matrix follows Codex. Linux uses bubblewrap plus seccomp, and
bubblewrap is already available in the development environment. WSL2 reuses
the Linux driver. macOS Seatbelt and Windows MXC drivers come after M1.
Network access is off by default. When enabled, it goes through a
domain-allowlist proxy; deny rules win, and local or private destinations are
blocked unless configured otherwise.

The session header records the initial mode. Switching modes is an explicit
user action and appends a durable event. Agents cannot change their own mode.
`free` requires an explicit flag or confirmation and cannot be granted by
project policy. A future managed policy layer can disable it.

### D20: Resume

Session storage:

```
~/.local/state/alpha-harness/sessions/<uuidv7>/
  header.json        identity, workspace/location binding, format version
  session.v0.jsonl   append-only durable events, one per line
  session.vN.jsonl   successor generation published only on write-open
  session.lock       writer lease (pid plus boot id)
```

The storage layer guarantees:

- Opening a session for reading applies migrations in memory and writes
  nothing.
- Repair and migration write a new generation beside the unchanged source.
  Existing generations are never renamed or deleted.
- One writer holds the lease. A stale lease can be taken over after
  confirmation.
- Recovery drops a torn tail line, closes interrupted turns with a durable
  `turn/end` repair event, and records missing tool results as interrupted.
  Assistant text that was never delivered before a hard crash is not
  reconstructed.
- Model history and the inbox are rebuilt from the log.

`alpha run --resume <id>` opens an idle session and shows its last state.
`--continue`, or new input, claims pending inbox work and resumes unfinished
tasks. `session.attach { afterSeq }` joins the event stream, replays durable
events, then releases live events that arrived during replay before returning
to normal delivery. Read-only observers such as a TUI therefore see an ordered
stream without gaps.

### D21: Testing

- Golden replay: the same log produces identical model history and
  projections.
- Crash injection: random truncation and torn lines recover with only the
  unsettled tail lost.
- Migration tests: read-open writes nothing; write-open publishes a successor
  without touching the source bytes.
- Lease tests: second writer rejected, stale lease takeover.
- Provider cassette replay: HTTP traffic recorded once and replayed
  deterministically.
- Protocol end to end: a local TCP carrier runs `alpha-protocol` against the
  managed core while an injected HTTP seam returns a deterministic SSE stream.
- Negative build tests: missing requirement, duplicate singleton, invalid
  override, and static registry collision each fail `cargo check` with
  actionable diagnostics.
- Build read-only snapshot test: a build changes only `target/`.

### D22: Prompt assembly

The prompt module assembles one deterministic system prompt from ordered
sections. Framework, policy, output, tool, project, capability, and persona
sections have stable keys and priorities. Persona is rendered last. Tool
schemas remain the canonical tool descriptions and are not copied into the
prompt. Runtime values such as the permission mode and workspace binding are
not rendered into the system prompt.

Project instructions are discovered through the filesystem seam from
`AGENTS.md` and `.alpha/AGENTS.md` between the workspace root and fixed cwd.
They are size-limited and labeled with their source path.

The complete prompt is recorded as a durable `system_prompt_set` event before
the first turn that uses it. Resume and replay use that snapshot instead of
rendering a new prompt in the background. Prompt changes therefore remain
explicit session facts.

Dynamic context sections render into `context_injected` user messages. The
runtime section is injected before the first user message of a session and is
re-injected only when its rendered content changes. The projection ignores an
older context message when compaction hides it, causing the current snapshot to
be injected again. This keeps the stable system prefix intact while making
later environment changes explicit and append-only.

### D23: Context compaction

The compaction module is an optional pre-turn context transformation. It uses a
replaceable token estimator and conservative defaults because exact provider
usage is not part of the event contract. The active context window comes from
provider model metadata when available; `context_window_tokens` is only the
fallback for providers that do not report one. When the configured threshold is
exceeded, it summarizes an older, complete turn prefix and keeps recent raw
history.

The default `body_after_prefix` scope counts growth after the stable system and
compacted-history prefix, avoiding repeated rewrites when a large carried prefix
is still below the model's hard limit. `total` counts the full active context.

The model-generated summary is produced without tools. If summarization fails,
the compactor commits a bounded deterministic excerpt instead of leaving an
oversized history untouched. `context_compacted` records the resulting summary
and the durable sequence range it replaces. History rebuilding inserts the
summary before records after that range.

### D24: Streaming

`ModelProvider::stream` is the default turn path. A provider without native
streaming falls back to `complete`, so the contract remains incremental
without requiring every adapter to implement SSE.

The HTTP seam exposes response bodies as byte streams. The OpenAI-compatible
adapter parses SSE text and tool-call fragments. The loop emits `agent/*` live
events for steps, text deltas, thinking deltas, tool lifecycle, completion,
and errors. It then commits one canonical assistant message and the completed
tool calls to the durable log. Thinking deltas are emitted separately from
visible text. If a stream fails, partial thinking and text are preserved as
structured interrupted content.

The CLI subscribes to live events: stdout receives assistant deltas and stderr
receives progress. Protocol stdio mode keeps stdout protocol-only.

### D25: Front and core

The public `alpha` binary is the only user entry point. `alpha-core` and
`alpha-front` are internal crate layers, not separate executables. A front
supervisor starts one hidden `alpha __core --stdio` child, performs the
`body.hello` handshake, and manages that child's lifecycle.

All front modes (`run`, `chat`, `tui`, and `sessions`) use the same
managed-core path. `alpha run`, `alpha chat`, and `alpha tui` start their own
core process. `alpha sessions` uses the managed core to read durable state.
Each front and core pair is independent. A front never attaches to another
instance's core, and a second writer for the same session fails with
`session_busy`.

Core startup defaults to three retries after the first attempt. A runtime crash
uses the same policy. A retry restarts the core and restores durable session
state, but it never replays an interrupted turn automatically. The hidden core
command exists for manual debugging and is not a supported user workflow.

The core caches Location contexts by `(workspace root, fixed cwd)`. Sessions in
the same Location share filesystem, process, prompt, and compaction services.
Each session actor adds its own mode, tools, approval channel, steer queue, and
provider override. The shared front runtime ignores events from sessions other
than the active one.

`alpha-front-runtime` owns frontend state and behavior that does not depend on
a UI toolkit. This includes session lifecycle, turn submission, steering,
cancellation, mode and route changes, provider configuration, model selection,
transcript reduction, interaction timeouts, slash-command behavior, and
recovery after a core restart. It depends on the protocol client, supervisor,
and schema, not on a terminal or desktop library. Frontends consume typed
state and events instead of parsing protocol JSON themselves.

Provider management is a shared runtime workflow backed by
`provider.list_kinds`, `provider.list_instances`, and
`provider.credential_status`. The runtime can create and remove instances,
choose the default instance, store an API key through the credential module,
validate an instance, edit provider configuration as JSON, refresh provider
state, and confirm a removal. Provider reads never return secrets, and secrets
do not remain in front state after submission. A new chat starts as an
in-memory draft. The runtime calls `session.create` only when the first
non-empty message is submitted.

`alpha-front-tui` handles terminal input, layout, rendering, selection,
clipboard support, and terminal lifecycle. The `chat` profile stays in the
current terminal, while `tui` enters the alternate screen. Both consume the
shared runtime.

The chat screen shows the effective provider/model route, reasoning effort, and
permission mode as `provider/model effort | mode`. `Ctrl-L` selects a
configured provider and model, `Shift-Tab` cycles the model's configured
reasoning effort, and `Ctrl-T` opens the effort picker. `/model`, `/effort`,
and `/thinking` provide the same controls. Route changes affect the next model
step and are durable, so resume restores the effective provider, model, and
reasoning effort. Selecting a model also stores it as the provider instance's
default model and stores the provider as the default, so new sessions start
with the same route. Thinking content is rendered separately from assistant
text without role prefixes. Assistant Markdown supports GFM tables. User
messages use a full-width dark-gray band with one row of padding above and
below on a near-black blue canvas. The transcript has no frame, title, or
empty-state placeholder. Dragging over transcript text highlights the
selection and copies it to the terminal host clipboard with OSC 52 when the
user releases the mouse.
Messages and slash commands share one multiline composer with command and
argument completions. `Enter` sends, `Ctrl-Enter` inserts a newline, and the
arrow keys, `Home`, `End`, `Backspace`, and `Delete` move the cursor or edit
text. `Ctrl-R` opens a session manager that shows the first user message as a
preview, switches sessions with the keyboard, and deletes idle sessions after
confirmation. A session cannot be deleted while a turn is running.

### D26: Credential storage

Provider secrets are stored only through the credential module. The module
exposes one `CredentialStore` service backed by Secret Service on Linux,
Keychain on macOS, or Credential Manager on Windows. Tests use an in-memory
backend.

Provider instances store only an opaque `auth_ref`. They never contain secret
values, environment-variable names, or platform-specific credential details.
If the keyring backend is unavailable, credential operations fail closed and
the front reports `keyring_unavailable`. There is no plaintext-file fallback.

The workspace MSRV is Rust 1.88 because the credential stack uses
`keyring-core` and platform-specific store crates.

### D27: Provider API adapters

Provider kinds and API adapters are separate module layers. A provider kind
owns authentication, provider configuration, model metadata, compatibility
options, and request hooks. An API adapter owns one wire protocol: message
conversion, streaming, tool-call accumulation, usage normalization, and
provider error mapping.

Adapters register under the global `api.adapter` registry capability. Provider
modules declare required and optional adapter keys and choose an adapter for
each instance or model. Adapter crates do not depend on provider modules, and
provider modules do not copy an adapter's protocol implementation.

The first provider set includes generic OpenAI Chat Completions, OpenAI
Responses, and Anthropic Messages kinds, along with dedicated DeepSeek and
OpenCode Go kinds. DeepSeek uses the Anthropic Messages adapter so
provider-native web search follows the same request path.

Model catalogs are layered. The core uses the provider's `/models` endpoint
with the stored credential when it can. Known ids keep their static metadata,
while newly discovered ids receive fallback metadata. Static provider metadata
remains the offline fallback. `model_overrides` in the user TOML can change a
model name, adapter, context window, output limit, reasoning flag,
`reasoning_levels` (or `thinking_levels`), provider tools, or default model.

Provider requests carry the selected reasoning level. OpenAI Chat Completions
and Responses map it to `reasoning_effort` and `reasoning.effort`; Anthropic
Messages maps it to a thinking mode and token budget. `auto` omits the explicit
control.

OpenCode Go requests also receive the durable session id in
`x-opencode-session`. This is the provider's required per-conversation routing
header; an explicitly configured header still takes precedence.

If provider config migration fails, the previous config is kept, the instance
is marked disabled, and provider construction rejects it until the user
repairs or replaces the configuration.

The hidden core exposes provider kind discovery, instance CRUD, validation, and
credential operations through `alpha-protocol`. Runtime instances live in the
global provider instance registry. The configuration layer persists that
projection to user TOML without changing the protocol surface. The core
creates `auth_ref` values and removes credentials when an instance is removed.
If keyring cleanup fails, the instance is still removed and the front reports
the cleanup warning.

Session loops hold a dynamic provider handle instead of a provider built once
at session creation. Updates to a provider instance therefore appear on the
next model step. A session does not need to restart when an endpoint, header,
or adapter changes.

Provider instances are persisted under `[providers.instances.<id>]` in the
user configuration file. Writes use last-write-wins and an atomic rename. The
in-memory registry is updated after the write succeeds.

### D28: Structured assistant content

Assistant messages are durable sequences of `AssistantContentBlock` values:
text, thinking, local tool calls, provider tool calls, provider tool results,
and citations. Local tool calls also produce separate `ToolCall` and
`ToolResult` events. Provider-native tools stay inside the assistant message
and never enter the local tool registry.

Adapters preserve provider thinking when replaying history. OpenAI Chat
Completions sends assistant `reasoning_content`. OpenAI Responses sends
`reasoning` items with encrypted content. Anthropic Messages sends signed
`thinking` blocks and falls back to text when a signature is unavailable.
Provider-facing history therefore keeps the structured content exposed by the
session log and frontends.

Schema 1 assistant messages with a plain `text` field are migrated into a
single text block when read. The current durable schema is version 7.
Provider-native tools, including Anthropic `web_search`, use provider tool
blocks, so the transcript and replay path do not need provider-specific event
types.

Each session has an effective provider/model route and thinking level. The
initial route comes from session creation metadata. Later changes append
`route_changed` events and apply to the next model step. Resume rebuilds the
route from the durable projection.

Before each model step, the loop appends a durable `request_header` event with
the provider, model, thinking level, local tool names, and provider-native tool
names. That event is the capability snapshot for the request.

`session.set_capabilities` can override provider-native tools for one session.
The override is recorded as a durable `capabilities_set` event and applies to
later model requests.

Session actors stay resident only while active or recently used.
`alpha-module-agent-host` evicts idle actors. A later `session.resume` rebuilds
the projection from the durable log and starts a new actor.

Steers sent outside an active turn remain pending until
`session.confirm_steers` accepts or discards them. Accepted steers start a new
turn. Discarded steers are removed without entering model history. The pending
steer queue is bounded and rejects overflow. Queued and settled steer
identities are durable, so a resumed actor rebuilds the pending set from the
log and waits for confirmation before executing anything. Discarding returns
the removed messages to the frontend, allowing an active queued steer to be
restored to the composer instead of being injected.

## 4. Runtime contract sketch

A module implements the `Module` contract. It declares its requirements and
provisions, defines a typed config, and provides an `activate` function that
registers services, tools, listeners, and tasks. Every registration returns a
disposer owned by the module scope. Deactivation unwinds the disposers in
reverse order.

Capabilities declare a scope level (global, location, or agent) and a kind
(singleton or registry). The composition binds singleton providers; modules do
not bind them directly. Registry entries receive their names when registered.

The sketch below is not a final API:

```rust
pub trait Module: Send + Sync + 'static {
    type Requirements;
    type Config: serde::de::DeserializeOwned + Default;
    const KEY: &'static str;

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError>;
}
```

## 5. Crates

| Crate | Responsibility |
| --- | --- |
| `alpha-kernel` | context, services, events, effects, scopes, structured spawn |
| `alpha-schema` | public shapes, durable event payloads, protocol method types |
| `alpha-protocol` | JSON-RPC 2.0 and NDJSON framing, request routing |
| `alpha-front-client` | blocking alpha-protocol client used by fronts |
| `alpha-front-supervisor` | managed core child process lifecycle |
| `alpha-front-runtime` | UI-independent frontend state, controller, and workflows |
| `alpha-front-tui` | interactive chat and full-screen terminal adapters |
| `alpha-provider-api` | provider descriptors and API adapter contracts |
| `alpha-api-openai-chat` | OpenAI Chat Completions adapter |
| `alpha-api-openai-responses` | OpenAI Responses adapter |
| `alpha-api-anthropic-messages` | Anthropic Messages adapter |
| `alpha-module-provider-openai-chat-completions` | generic OpenAI Chat Completions provider |
| `alpha-module-provider-openai-responses` | generic OpenAI Responses provider |
| `alpha-module-provider-anthropic-messages` | generic Anthropic Messages provider |
| `alpha-module-provider-deepseek` | DeepSeek provider |
| `alpha-module-provider-opencode-go` | OpenCode Go provider |
| `alpha-module-provider-config` | user TOML persistence for provider instances |
| `alpha-macros` | `#[alpha::module]` and composition-related macros |
| `alpha-build` | dependency-graph validation and module table generation |
| `alpha-module-config` | layered TOML configuration and provenance |
| `alpha-module-credential` | cross-platform keyring-backed credential storage |
| `alpha-module-prompt` | deterministic system-prompt assembly, project instructions, and dynamic context injection |
| `alpha-module-compaction` | budget estimation and durable context compaction |
| `alpha-module-session` | append-only session log, projections, migrations |
| `alpha-module-loop` | default agent driver (replaceable) |
| `alpha-module-fs` | filesystem seam and local driver |
| `alpha-module-process` | execution seam and local driver |
| `alpha-module-http` | network seam and local driver |
| `alpha-module-permission` | policy, approval flow, sandbox integration |
| `alpha-module-tools` | `read`, `grep`, `write`, `edit`, `shell` |
| `alpha-module-provider-openai` | OpenAI-compatible provider adapter |
| `alpha-bundle-default` | dependencies only: the default M1 composition |

## 6. M1 scope

The M1 scope includes:

- Kernel primitives, schema, protocol, and build-time composition checks.
- Session log with resume, lease, repair, and projections.
- The default loop, an OpenAI-compatible provider, and local fs/process/http
  drivers.
- Tools: `read`, `grep`, `write`, `edit`, `shell`.
- Permission modes `read`, `auto`, `reviewed`, and `free`; policy files;
  protected paths; durable audit events; and the Linux sandbox driver
  (bubblewrap plus seccomp).
- `alpha run` and `alpha serve --stdio` sharing one JSON-RPC/NDJSON schema.

The following remain outside M1: macOS and Windows sandbox drivers, MCP,
subagents, the TUI, multi-Location concurrency, remote drivers, and managed
policy.

Acceptance criteria:

1. `alpha run "..."` completes a real file-editing task through the seams.
2. `alpha run` and `alpha serve --stdio` share the same schema; stdout is
   protocol-only.
3. Resume works end to end, including lease, repair, and `attach`.
4. Permission modes behave as specified; prompts offer deny, once, session.
5. The four negative composition cases fail `cargo check` with actionable
   diagnostics.
6. Swapping a loop or provider dependency re-validates and builds.
7. Provider cassette replay reproduces the same session log.
8. Builds change only `target/`.

## 7. Milestones

- **M1**: everything in section 6.
- **Post-M1 A/B**: prompt snapshots, project instructions, context compaction,
  provider streaming, agent live events, and streamed CLI output.
- **M1.5**: model-backed reviewer providers, macOS Seatbelt and Windows MXC
  sandbox drivers, reviewer policy files.
- **M2**: TUI profile, MCP client module, subagents, multi-Location daemon.

## 8. Open items

- Reviewer model selection and cost policy
- Project trust UX for project-level policy files
- Managed policy layer
- MSRV policy
- Protocol method catalog and versioning details
