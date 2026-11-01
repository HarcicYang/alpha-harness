# Composition decoupling — status and remaining work

This note records where the fixed-layer decoupling stands so the work can be
resumed without re-deriving the plan. It is deliberately specific: every item
names the file and symbol involved.

## Goal

`apps/alpha-harness/src/main.rs` must not name any concrete `alpha-*` crate, so
that trimming or replacing a capability is a `Cargo.toml` change and nothing
else. Capability replacement means: remove the old implementation crate, add the
new one. Two implementations of the same singleton capability may not coexist;
`alpha_build::check()` rejects that at build time.

## Done

| Commit | Change |
| --- | --- |
| `e09d93a` | `alpha-module-session` → `alpha-session-log` (infrastructure crate, no module metadata) |
| `75fb2bf` | `alpha-host-api` contracts; module activation metadata (`scope`, `module`, `config`, `requires_config`); generation of a callable `alpha_composition.rs` with `ALPHA_MODULES` (topologically ordered per scope) and `ALPHA_FRONTS` |
| `3752ca7` | Front entry points: `__front_run` (`alpha-front-runtime`), `__front_chat`/`__front_tui` (`alpha-front-tui`), `__front_desktop` (`alpha-front-desktop`) |
| `759621f` | Contract keys stay in the generated module key list |
| `811a6ba` | Coexisting providers are a build error; the `overrides` mechanism is gone |
| `4bc3806` | `alpha-bundle-default` deleted; the composition root's direct dependencies are the module set |
| `d718fc0` | `alpha-credential-api`, `alpha-http-api` contracts; API adapters and `alpha-provider-api` depend on the contracts, not the drivers |
| `eb3f357` | `ProviderConfigError`, `ConfigFile`, `ProviderConfigStore` moved into `alpha-provider-api` |
| `8126ff5` | `alpha-prompt-api`: `PromptAssembler`, `PromptError` |
| `22b9a71` | `alpha-permission-api`: requests, approvals, audit, `PermissionService` |
| `891058e` | `alpha-agent-api`: providers, routes, registries, `AgentDriver`, turn errors |
| `43ddd9b` | `apps/alpha-harness/src/cli.rs`: clap **builder** driven by `Host::fronts` |
| `75bf502` | `main.rs` is four lines with no `alpha-*` import; the generated `ALPHA_HOST` supplies every command |

Current invariant: `cargo check --workspace --all-targets` passes and the tree is
clean.

## Remaining

### 1. Contract crates — done

All six contracts exist: `alpha-credential-api`, `alpha-http-api`,
`alpha-provider-api` (extended), `alpha-prompt-api`, `alpha-permission-api`,
`alpha-agent-api`. Each implementation crate keeps `pub use` re-exports, so
existing imports and tests are unchanged.


| Crate | Move out of | Symbols |
| --- | --- | --- |
| `alpha-prompt-api` | `alpha-module-prompt/src/lib.rs` | `PromptAssembler` (line ~136), prompt section/context registry types |
| `alpha-permission-api` | `alpha-module-permission/src/` | `PermissionService`, `PermissionRequest`, `PermissionOutcome`, `GrantScope`, `ApprovalChannel`, `ApprovalPrompt`, `ApprovalDecision`, `AuditSink`, `AuditRecord`, `DecisionSource`, `IntentClassifier`, `ApprovalReviewer`, `Policy` |
| `alpha-agent-api` | `alpha-module-loop/src/agent.rs`, `alpha-module-agent-host/src/lib.rs` | `AgentDriver`, `ModelProvider`, `ModelRoute`, `ModelRouteOverride`, `ProviderRegistry`, `ProviderToolsOverride`, `SteerQueue`, `SessionRuntime`, `SessionRuntimeFactory`, `SessionRuntimeRequest` |

Each implementation crate keeps `pub use alpha_<capability>_api::*;` so existing
imports and tests keep compiling.

Acceptance: `grep -rn "alpha_module_loop\|alpha_module_permission\|alpha_module_prompt" apps/alpha-harness/src` returns nothing.

### 2. Generated slot assertions

After activating a scope, the generated code should assert the capability slots
the shell consumes exist, for example:

```rust
assert_service::<dyn AgentDriver>(ctx, "agent.loop")?;
```

Emitted from `crates/alpha-build/src/generate.rs`; the helper lives in
`alpha-host-api` and returns an error naming the missing capability instead of
failing later with a bare "service not found".

### 3. `alpha-core` — done

The core server (composition, session runtime, router) lives in
`crates/alpha-core`. It depends on module crates but not on any frontend crate,
so a composition root decides which fronts exist.

`apps/alpha-min` is the working proof: it depends on `alpha-core` plus one
provider kind and the headless `run` front. Its `--help` lists only
`run` and `sessions`, and `cargo tree -p alpha-min` contains no `gpui`,
`ratatui`, or `crossterm`. CI checks both.

### 3a. `alpha-shell` — superseded

`apps/alpha-harness/src/cli.rs` already holds the fixed shell: clap command
builder over `Host::fronts`, dispatch, `sessions`, and `__core --stdio`. `main.rs`
no longer names any `alpha-*` crate.

Still inside `apps/alpha-harness/src/lib.rs` is the core server (router, session
runtime factory, ~2070 lines). Extracting it into a crate that depends only on
the contracts is what would let a second composition root (`apps/alpha-min`)
avoid linking the desktop front. Until then, keeping the CLI here is enough for
the trimming goal, because the generated table is per-crate.

The remaining extraction is:

- CLI built with the `clap` builder API from `Host::fronts` plus `sessions` and
  the hidden `__core --stdio` command (no `#[derive(Subcommand)]` enum, because
  commands are data).
- `serve_*` and the JSON-RPC router.
- The session runtime factory, activating Location and Agent modules by walking
  `Host::modules_at(scope)` with `RuntimeValues`.
- Dependencies limited to `alpha-kernel`, `alpha-schema`, `alpha-protocol`,
  `alpha-session-log`, `alpha-host-api`, `alpha-provider-api`,
  `alpha-agent-api`, `alpha-permission-api`, `alpha-prompt-api`,
  `alpha-credential-api`, `alpha-http-api` and `clap`.

### 4. Composition root

`apps/alpha-harness`:

```rust
// src/main.rs
fn main() -> std::process::ExitCode {
    alpha_shell::run(alpha_composition::host())
}
```

- `src/lib.rs` keeps `include!(concat!(env!("OUT_DIR"), "/alpha_composition.rs"));`
  and `pub use alpha_shell::*;`.
- The six integration tests move to `crates/alpha-shell/tests/`.

### 5. Documentation

- `docs/architecture.md`: remove the stale `alpha_kernel::bootstrap!()` claim
  (line ~101); document that the shell holds no module references.
- `docs/usage.md`: state that the command list follows the build.
- `CONTRIBUTING.md`: explain trimming and replacement (delete a dependency line,
  add a replacement crate, rebuild; the build fails if a capability is left
  without exactly one provider).

### 6. Templates and CI

- `apps/alpha-default`: the current dependency list, as a copyable composition
  root.
- `apps/alpha-min`: run + tui only, no desktop, no provider kinds beyond one.
- CI jobs:
  1. `alpha-min` builds and `alpha --help` has no `desktop`; `cargo tree -p
     alpha-min` has no `gpui`.
  2. A fixture that replaces `alpha-module-loop` with a stub crate providing
     `agent.loop`; the build succeeds.
  3. A fixture that keeps both loop crates; the build fails with
     `SingletonConflict`.

## Notes

- `alpha-build` still supports `kind = "bundle"` for grouping, but no bundle
  exists; the code path is kept for future aggregation crates.
- `crates/alpha-build/tests/fixture.rs` builds temporary workspaces and runs
  `cargo check` on them; it is the natural place for the replacement/trimming
  fixtures in item 6.
