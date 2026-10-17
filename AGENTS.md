# Repository Instructions

These instructions apply to the entire repository.

## Architecture

- Keep the kernel limited to the component runtime and basic terminal/stdio IO.
- Put every substantial behavior in a module with explicit capabilities.
- Preserve the compile-time Cargo composition model. Do not add runtime dynamic
  library loading or WASM hosting.
- Keep the session log as the source of truth. Any model-visible context must be
  reconstructible from durable events.
- Keep UI-independent frontend state and behavior in `alpha-front-runtime`.
  Session control, turn actions, provider workflows, command semantics,
  transcript reduction, restart recovery, and interaction timeouts must not
  live in a terminal, desktop, or web adapter.
- Keep protocol messages and shared front DTOs in `alpha-schema`. Frontends must
  consume typed events and state instead of parsing protocol JSON ad hoc.
- Treat frontend adapters as presentation and input layers. TUI code owns
  terminal rendering, layout, key/mouse mapping, selection, clipboard, and
  terminal lifecycle only; future GUI and WebUI adapters must follow the same
  boundary.
- Do not make a frontend crate depend on core service crates such as
  `alpha-provider-api`. Shared provider and session shapes belong in
  `alpha-schema`; shared frontend workflows belong in `alpha-front-runtime`.
- Do not introduce JavaScript, TypeScript, Node.js, npm, pnpm, or JS-based build
  tooling anywhere in the repository or shipped dependency chain.

## Documentation

- Write repository documentation in English.
- Keep `docs/architecture.md` as the single source of truth for architecture.
- Document current behavior directly. Do not add superseded design sections.

## Workflow

- After completing each small, self-contained feature, create a commit.
- Use an English Conventional Commit subject with no body.
- Keep each commit scoped to one logical change.
- Before committing, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Do not write credentials, API keys, or tokens into repository files.
