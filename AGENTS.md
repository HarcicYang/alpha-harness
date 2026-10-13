# Repository Instructions

These instructions apply to the entire repository.

## Architecture

- Keep the kernel limited to the component runtime and basic terminal/stdio IO.
- Put every substantial behavior in a module with explicit capabilities.
- Preserve the compile-time Cargo composition model. Do not add runtime dynamic
  library loading or WASM hosting.
- Keep the session log as the source of truth. Any model-visible context must be
  reconstructible from durable events.
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
