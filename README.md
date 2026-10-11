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
- Assistant output streams to stdout as the model produces it. Step, tool, and
  error progress stays on stderr.

## Prompting and context

- `AGENTS.md` and `.alpha/AGENTS.md` files are discovered from the workspace
  root through the fixed cwd and added to the system prompt.
- The complete prompt is recorded in the session log and replayed on resume.
- Long histories are compacted into a durable summary before the configured
  context budget is exhausted. Compaction failures leave the original history
  untouched.

## Sessions

```sh
alpha sessions                        # list stored sessions
alpha run --resume <SESSION_ID>       # reopen idle and repair interrupted work
alpha run --resume <SESSION_ID> "continue"
```

## Common commands

```sh
cargo build       # Build
cargo run -- --help                                  # CLI help
cargo run -- run --mode free "add a README section"  # One prompt (needs OPENAI_API_KEY)
cargo test        # Test
cargo fmt         # Format
cargo clippy      # Lint
```
