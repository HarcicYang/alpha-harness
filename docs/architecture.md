# alpha-harness Architecture

Status: M1 design freeze. This document is the single source of truth for the
architecture.

## 1. Vision

alpha-harness is a lightweight coding agent harness. It takes the opposite
route from feature-heavy harnesses: a minimal kernel plus atomic modules, in
the spirit of NixOS composability.

Two core ideas:

1. Keep the heavy harness light. The kernel stays tiny and every substantial
   feature is opt-in.
2. Everything is a module. Every capability is atomic, self-contained, and
   independently replaceable.

## 2. Hard principles

- **P1: Substantive behavior lives in modules.** The kernel contains only the
  component runtime plus basic stdio. Anything that creates user-visible
  behavior (agent loop, providers, tools, sessions, permissions, UI) is a
  module.
- **P2: No JavaScript or TypeScript ecosystem.** No Node.js, npm, pnpm, or JS
  tooling in the build chain, documentation tooling, shipped modules, or
  runtime dependencies. Protocols (JSON-RPC, MCP) are language-neutral and
  remain allowed.

## 3. Architecture decisions

### D1: Kernel scope

The kernel is a component runtime: context and service registry, declared
dependencies (reactive coeffects), typed events, reversible effects, scopes and
lifecycle, and scope-bound task spawning. Its only IO is terminal and stdio.

### D2: Agent loop is a module

The default ReAct loop is the first shipped module, not kernel code. Consumers
depend on the `AgentDriver` trait, and the default `AgentLoop` implementation
registers itself under the same service key, so a composition can mount a
different driver in its place. Each turn receives its agent-scoped `Ctx` so
progress is emitted as typed live events rather than through a loop-specific
observer callback.

### D3: Compile-time composition

Modules are Rust crates. Composition happens at compile time. There is no
runtime plugin loader, no dynamic library ABI, and no WASM host in M1.

### D4: Module granularity

A capability is the atomic unit of replacement. A module may own any number of
capabilities (tools, adapters, policies) as long as every capability has a
stable key and can be enabled, disabled, and replaced independently. A module
boundary must never become a replacement boundary. Bundles are pure
composition: dependencies only, no behavior.

### D5: Singleton and registry capabilities

- **Singleton capabilities** have exactly one provider. A second provider is a
  build error unless the composition declares an explicit override.
- **Registry capabilities** (tools, adapters, commands) accept multiple
  providers keyed by name. Static name collisions are build errors; dynamic
  sources use namespaces (for example `mcp__<server>__<tool>`).

### D6: Composition source is the Cargo dependency graph

The app declares modules as ordinary dependencies in `Cargo.toml`. Each module
crate declares its capabilities in `[package.metadata.alpha]`. The app's
`build.rs` calls `alpha_build::check()`, which:

1. reads the resolved dependency graph and enabled features,
2. validates the composition (missing requirements, singleton conflicts,
   invalid overrides, static registry name collisions, metadata version),
3. generates an explicit module table into `OUT_DIR`.

`main.rs` includes the generated table via `alpha_kernel::bootstrap!()`.
Build-time tooling is strictly read-only with respect to user files: it writes
only to `OUT_DIR` and reports problems as diagnostics. It never edits
manifests or sources; suggested fixes are for the user to apply. A future
`alpha fix` helper, if ever added, must default to dry-run and require explicit
confirmation.

### D7: Options are runtime configuration

Composition and module presence are compile-time. Tunable values (model names,
timeouts, limits) live in TOML with layered precedence:

module defaults < user config < project config < environment < CLI flags.

Modules declare a serde schema plus light metadata (docs, secret flag,
restart-required flag, scope) for CLI, TUI, and documentation. Unknown keys are
rejected. Effective values keep provenance so `alpha config show` can report
which layer set a value.

### D8: The session log is the single source of truth

Model-visible means logged. Everything the model can observe must be
reconstructible from durable, append-only session events. UI, statistics,
resume, fork, compaction, and telemetry are projections over the log. New
model-visible behavior requires new event types with versioned migrations.
Compaction appends new events; it never rewrites history.

### D9: IO boundaries

The kernel provides stdio only. Filesystem, process execution, and network
access are seam modules with swappable drivers:

- filesystem: local driver first; remote-workspace, memory, unavailable later
- process: local driver first; remote and unavailable later
- network: local HTTP driver first; record/replay driver for tests

Modules must not call `std::fs`, `std::process`, or sockets directly (contract
plus CI lint). Tools assert permissions explicitly; the tool registry performs
no implicit permission wrapping. Authority follows capability capture: trusted
built-ins capture powerful services, application tools receive narrow
capabilities.

### D10: Scopes

Runtime scopes form a chain: **Global, Location, Agent, Call**.

- Global: composition-level singletons (telemetry, credentials store).
- Location: project-scoped service graph (filesystem root, permission policy,
  execution world, project config, watchers, session storage).
- Agent: one session/agent (loop, model route, session state, tool overlay).
- Call: cancellation tokens and ephemeral effects.

Location graphs are built lazily and may be reclaimed when idle.

### D11: Workspace determines cwd

A workspace is the execution placement: a local root or a remote target. Nothing
below the workspace may invent its own cwd:

- Location cwd is fixed at creation (workspace root or a subdirectory).
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
- `alpha __core --stdio`: hidden core process managed by a front.
- Rust SDK embeds the core crates directly.

A TUI is the second profile; a daemon can come later because protocol and
transport are separate.

### D13: External protocol

JSON-RPC 2.0 over stdio with newline-delimited JSON framing. stdout carries
protocol messages only; logs go to stderr. The `alpha-schema` crate owns
public shapes and durable payloads. The first carrier is the private stdio
pipe between a front supervisor and its core child. Other carriers can be
added later without protocol changes.

Methods include session lifecycle (`session.create`, `session.list`,
`session.resume`, `session.attach`, `session.events`), message submission,
cancellation, and approval requests (`permission.request` as a server-initiated
request).

### D14: Events

Two classes:

- **Durable events**: session facts, serde-serializable, versioned, carrying a
  monotonically increasing `seq`. Used for resume, replay, projections, and
  telemetry.
- **Live events**: process-local observation and interception. They may carry
  non-serializable values (handles, cancellation tokens) and may be dropped.
  Any live state must be reconstructible from durable events.

Five dispatch modes, part of each event's type contract and enforced with
sealed marker traits: `emit`, `waterfall`, `parallel`, `serial`, `bail`.
Event domains are namespaced: `session/*`, `agent/*`, `tools/*`, `fs/*`,
`perm/*`. Listener registration is a reversible effect owned by a scope.

### D15: Concurrency and writing

- One driver task (actor) per agent with a bounded mailbox. Agent state is
  mutated only inside the task.
- One writer per session. The driver owns the write handle; the session module
  flushes batches through an internal writer task. Sequence numbers are
  allocated at append time.
- Tool calls run with bounded parallelism (default 4). Tools declare
  `parallel-safe` or are exclusive by default; exclusive calls act as ordering
  barriers. Durable results commit in call order; live progress is immediate.
- Cancellation is a token tree: scope, turn, step, call.
- Background work must use `ctx.spawn` (scope-bound, cancelled and joined on
  scope close). Raw `tokio::spawn` is banned by lint.

### D16: Repository layout

A Cargo workspace. The app package `alpha-harness` lives in
`apps/alpha-harness` and produces the `alpha` binary. Modules and libraries
live under `crates/`. The toolchain is pinned in `rust-toolchain.toml`.

### D17: Module distribution

Modules are ordinary crates distributed through crates.io, git, or path
dependencies. `keywords` includes `alpha-module` for discovery. The kernel
API follows semver; during 0.x, breaking changes bump the minor version.
`Cargo.lock` is committed for the app. Third-party modules are trusted,
compiled code: capability discipline is contract plus lint, not a sandbox.
Supply-chain checks (cargo-deny, cargo-audit) run in CI.

### D18: Tools

One structural tool contract for built-ins and third-party tools alike:

- Typed input via serde plus schemars; typed machine output; non-empty
  model-visible content; optional UI metadata (three-value result).
- Errors are classified: `ToolFailure` (expected, model-visible),
  operational errors, and cancellation (never a tool result).
- Execution pipeline: per-request frozen tool snapshot logged in
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

`shell` replaces a dedicated bash tool. The available shell list is detected
per Location through the process seam, so remote workspaces report their own
capabilities. The tool description and schema list the available shells and the
default, and an unavailable or unknown shell produces a model-visible failure
listing valid options. Shell kinds are registry entries (`bash`, `zsh`, `fish`,
`sh`, `pwsh`, `powershell`, `cmd`, and third-party kinds) with per-kind argv
templates. The default shell resolves as: module config > `$SHELL` > platform
fallback. The tool is exclusive, asserts `shell.exec`, inherits the parent
environment with an optional denylist, cancels by killing the process group,
and streams stdout/stderr as tagged live progress.

### D19: Permission modes and sandbox

Four user-facing modes:

| Mode | Sandbox | Boundary behavior |
| --- | --- | --- |
| `read` | read-only, network off, no execution | mutate requests denied, no prompts |
| `auto` (default) | workspace-write plus temp dirs, network off, protected paths read-only | on-request synchronous approval |
| `reviewed` | same as `auto` | two-stage review chain, high risk prompts the user |
| `free` | full access, network on | no prompts |

M1 ships `read`, `auto`, and `free`. `reviewed` lands in M1.5.

Approval prompts offer three choices: deny, allow once, allow for this session.
There is no "always allow" button. Session grants are durable events and
survive resume; they expire with the session. Persistent grants exist only as
explicit, user-edited policy file changes.

Never auto-approved: critical-path deletions, credential files, policy and
sandbox configuration, push and publish/deploy operations, and cross-session
messaging.

`reviewed` decision order:

1. Static rules; deny wins; protected classes are intercepted.
2. Reads and workspace edits inside the sandbox: approved.
3. Claude-style intent classifier. Approval continues.
4. Otherwise Codex-style auto review: exfiltration, credential probing,
   persistent security weakening, destructive actions, risk tiers.
5. Low and medium risk proceed; high risk prompts the user; critical risk is
   denied; reviewer failures fail closed (the action does not run; the prompt
   is marked as review-unavailable and times out to denied).
6. Both verdicts and reasons are durable events.

The reviewer is a seam with providers: `user`, `auto`, `deny`, `policy-only`.
Reviewer requests contain the action, arguments, model-provided reason, target
paths and domains, and policy text. Raw tool output and repository content are
stripped to resist prompt injection.

Sandbox matrix follows Codex: Linux uses bubblewrap plus seccomp (bubblewrap is
already available in the development environment); WSL2 reuses the Linux
driver; macOS Seatbelt and Windows MXC drivers come after M1. Network access
is off by default; when enabled it goes through a domain-allowlist proxy where
deny wins and local or private destinations are blocked by default.

Mode governance: the mode is recorded in the session header; switching modes
is an explicit user action recorded as a durable event. Agents cannot switch
their own mode. `free` requires an explicit flag or confirmation and cannot be
granted by project policy; a managed layer (future) can disable it.

### D20: Resume

Session storage:

```
~/.local/state/alpha-harness/sessions/<uuidv7>/
  header.json        identity, workspace/location binding, format version
  session.v0.jsonl   append-only durable events, one per line
  session.vN.jsonl   successor generation published only on write-open
  session.lock       writer lease (pid plus boot id)
```

Guarantees:

- Read-open performs migrations in memory and writes nothing.
- Repair and migration publish a new generation beside the untouched source.
  Existing generations are never renamed or deleted.
- One writer holds the lease; a stale lease can be taken over after
  confirmation.
- Recovery drops a torn tail line, closes interrupted turns with a durable
  `turn/end` repair event, and records missing tool results as interrupted.
  Undelivered assistant text from a hard crash is not reconstructed.
- Projections (model history, inbox) are rebuilt from the log.

`alpha run --resume <id>` opens the session idle and shows the last state.
`--continue` (or new input) claims pending inbox work and resumes unfinished
tasks. `session.attach { afterSeq }` replays durable events and then streams
live events for read-only observers such as a TUI.

### D21: Testing

- Golden replay: the same log folds to identical model history and projections.
- Crash injection: random truncation and torn lines recover with only the
  unsettled tail lost.
- Migration tests: read-open writes nothing; write-open publishes a successor
  without touching the source bytes.
- Lease tests: second writer rejected, stale lease takeover.
- Provider cassette replay: HTTP traffic recorded once and replayed
  deterministically.
- Negative build tests: missing requirement, duplicate singleton, invalid
  override, and static registry collision each fail `cargo check` with
  actionable diagnostics.
- Build read-only snapshot test: a build changes only `target/`.

### D22: Prompt assembly

The prompt module assembles one deterministic system prompt from ordered
sections. Framework, policy, output, tool, project, capability, and persona
sections have stable keys and priorities, with persona rendered last. Tool
schemas remain the canonical tool descriptions and are not duplicated into the
prompt.

Project instructions are discovered from `AGENTS.md` and
`.alpha/AGENTS.md` between the workspace root and fixed cwd through the
filesystem seam. They are size-limited and labeled with their source path.

The complete prompt is recorded as a durable `system_prompt_set` event before
the first turn that uses it. Resume and replay use the snapshot from the log
instead of silently re-rendering a different prompt. Prompt changes therefore
remain explicit session facts.

### D23: Context compaction

The compaction module runs as an optional pre-turn context transformation. It
uses a replaceable token estimator and conservative defaults because exact
provider usage is not part of the event contract. Above the configured
threshold it summarizes an older, complete turn prefix while retaining recent
raw history.

The summary is generated without tools and committed only after it succeeds.
`context_compacted` records the summary and the durable sequence range it
replaces. History rebuilding injects that summary before records after the
range. A failed summarization call appends nothing and leaves the full history
intact.

### D24: Streaming

`ModelProvider::stream` is the default turn path. Providers without a native
streaming implementation fall back to `complete`, so the provider contract
remains incremental without forcing every adapter to implement SSE.

The HTTP seam exposes response bodies as byte streams. The OpenAI-compatible
adapter parses SSE text and tool-call fragments. The loop emits
`agent/*` live events for steps, text deltas, tool lifecycle, completion, and
errors, then commits one canonical assistant message and complete tool calls to
the durable log. Partial assistant text received before a failed stream is
preserved as an interrupted prefix.

The CLI subscribes to live events: stdout receives assistant deltas and stderr
receives progress. Protocol stdio mode keeps stdout protocol-only.

### D25: Front and core

The public `alpha` binary is the only user entry point. `alpha-core` and
`alpha-front` are internal crate layers, not separate executables. A front
supervisor starts one hidden `alpha __core --stdio` child, performs the
`body.hello` handshake, and owns the child lifecycle.

All front modes (`run`, `chat`, `tui`, `sessions`) use the same managed-core
path. Each front+core pair is an independent instance. A front never attaches
to another instance's core, and a second writer for the same session fails
with `session_busy`.

Core startup defaults to three retries after the initial attempt. Retries
restart the core process and restore durable session state only; an
interrupted turn is never replayed automatically. The hidden core command is
available for manual debugging but is not a supported user workflow.

### D26: Credential storage

Provider secrets are stored only through the credential module. The module
exposes one `CredentialStore` service over platform keyring backends:
Secret Service on Linux, Keychain on macOS, and Credential Manager on Windows.
Tests use an in-memory backend.

Provider instances store only an opaque `auth_ref`. They never store secret
values, environment-variable names, or platform-specific credential details.
If the keyring backend is unavailable, credential operations fail closed and
the front reports `keyring_unavailable`; there is no plaintext-file fallback.

The workspace MSRV is Rust 1.88 so the credential stack can use
`keyring-core` with platform-specific store crates.

### D27: Provider API adapters

Provider kinds and API adapters are separate module layers. A provider kind
owns authentication, provider configuration, model metadata, compatibility
options, and request hooks. An API adapter owns one wire protocol, including
message conversion, streaming, tool-call accumulation, usage normalization,
and provider error mapping.

Adapters register under the global `api.adapter` registry capability. Provider
modules declare required and optional adapter keys and select an adapter per
instance or model. Adapter crates do not depend on provider modules, and
provider modules do not copy adapter protocol implementations.

## 4. Runtime contract sketch

A module implements a `Module` contract: declared requirements and provisions,
a typed config, and an `activate` function that registers services, tools,
listeners, and tasks. All registrations return disposers owned by the module's
scope and are unwound in reverse order on deactivate.

Capabilities declare a scope level (global, location, agent) and a kind
(singleton or registry). Singleton providers are bound by the composition;
modules never bind singletons themselves. Registry entries are named at
registration.

Code sketch (not final API):

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
| `alpha-provider-api` | provider descriptors and API adapter contracts |
| `alpha-api-openai-chat` | OpenAI Chat Completions adapter |
| `alpha-api-openai-responses` | OpenAI Responses adapter |
| `alpha-api-anthropic-messages` | Anthropic Messages adapter |
| `alpha-macros` | `#[alpha::module]` and composition-related macros |
| `alpha-build` | dependency-graph validation and module table generation |
| `alpha-module-config` | layered TOML configuration and provenance |
| `alpha-module-credential` | cross-platform keyring-backed credential storage |
| `alpha-module-prompt` | deterministic system-prompt assembly and project instructions |
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

In scope:

- Kernel primitives, schema, protocol, build-time composition checks.
- Session log with resume, lease, repair, and projections.
- Default loop, OpenAI-compatible provider, fs/process/http local drivers.
- Tools: `read`, `grep`, `write`, `edit`, `shell`.
- Permission modes `read`, `auto`, `free`; policy file; protected paths;
  durable audit events; Linux sandbox driver (bubblewrap plus seccomp).
- `alpha run` and `alpha serve --stdio` sharing one JSON-RPC/NDJSON schema.

Out of scope for M1: `reviewed` mode, macOS and Windows sandbox drivers, MCP,
subagents, TUI, multi-Location concurrency, remote drivers, managed policy.

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
- **M1.5**: `reviewed` mode (classifier plus auto review), macOS Seatbelt and
  Windows MXC sandbox drivers, reviewer policy files.
- **M2**: TUI profile, MCP client module, subagents, multi-Location daemon.

## 8. Open items

- Reviewer model selection and cost policy.
- Project trust UX for project-level policy files.
- Managed policy layer.
- MSRV policy.
- Protocol method catalog and versioning details.
