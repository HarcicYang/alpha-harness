# alpha-harness

A lightweight, everything-is-a-module coding agent harness. Design decisions
live in [docs/architecture.md](docs/architecture.md).

## Layout

- `apps/alpha-harness`: the `alpha` binary.
- `crates/`: kernel, protocol, and modules.

## Build

```sh
cargo build --release
```

## Run one prompt

```sh
export OPENAI_API_KEY=...
alpha run --mode free --model deepseek-chat \
  --base-url https://api.deepseek.com/v1 "add a README section"
```

- `--mode read|auto|free` selects the permission mode (`auto` is the default).
- `--workspace <path>` selects the workspace root; it defaults to the current
  directory.
- Interactive terminals get approval prompts in `auto` mode. Non-interactive
  runs fail closed and need `--mode free` or explicit policy rules.
- Shell commands run inside a bubblewrap sandbox when it is available.

## Sessions

```sh
alpha sessions                        # list stored sessions
alpha run --resume <SESSION_ID>       # reopen idle and repair interrupted work
alpha run --resume <SESSION_ID> "continue"
```

## JSON-RPC over stdio

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"alpha.ping"}' | alpha serve
```

Methods: `alpha.ping`, `session.list`, `session.attach { sessionId, afterSeq }`.

## Common commands

```sh
cargo build       # Build
cargo run         # Run
cargo test        # Test
cargo fmt         # Format
cargo clippy      # Lint
```
