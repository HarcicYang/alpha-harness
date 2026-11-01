# Composition and replacement

Composition is part of the Cargo dependency graph. The app crate lists the
modules, providers, and frontends it contains. `alpha-build` validates that
graph and generates the activation table used by the binary.

There is no plugin loader, dynamic library host, or WASM runtime.

## What a package declares

A package declares its role in `[package.metadata.alpha]`:

| Field | Meaning |
| --- | --- |
| `api` | Metadata format version |
| `kind` | `module`, `contract`, `front`, or `bundle` |
| `key` | Stable module key |
| `scope` | `global`, `location`, or `agent` |
| `module` | Module type exported at the crate root |
| `config` | Configuration type exported at the crate root |
| `requires_config` | Config keys that must exist before activation |
| `provides` | Capability keys supplied by the package |
| `requires` | Capability keys consumed by the package |
| `registry_entries` | Static names placed in a named registry |
| `commands` | Commands contributed by a frontend crate |

A singleton capability has one provider in a composition. A registry
capability can have several named entries. Requirements are resolved before a
module activates.

Contract crates declare capability interfaces without an activation step.
Implementation crates use the contract and may re-export its public types.

## Generated composition

The build helper reads Cargo metadata and the resolved dependency graph. It
writes these files into `OUT_DIR`:

- `alpha_composition.rs`: activation code, ordered module table, capability
  keys, and frontend commands.
- `alpha_modules.rs`: module keys available to the composition.

The helper does not rewrite `Cargo.toml` or source files. A failed validation
stops the build before the binary links.

Validation rejects:

- A required capability with no provider.
- More than one provider for a singleton capability.
- Invalid package metadata.
- Static registry name collisions.
- Frontend commands that do not resolve to an entry function.
- Composition cycles or activation-order conflicts.

Frontends are data in the generated host. The CLI model does not contain a
hand-written match over every frontend.

## Remove a capability

Delete the implementation crate from the composition root's dependency list,
then rebuild:

```sh
cargo build -p alpha-harness
```

If another module still requires a capability supplied only by that crate, the
build fails and names the missing requirement. Otherwise the implementation,
its commands, and its transitive dependencies disappear from the binary.

`apps/alpha-min` is a checked example. It includes the core, one provider kind,
and the headless `run` frontend. Its help output contains `run` and `sessions`
and does not contain `desktop` or `tui`.

```sh
cargo build -p alpha-min
./target/debug/alpha-min --help
cargo tree -p alpha-min
```

The dependency tree for `alpha-min` must not contain `gpui`, `ratatui`, or
`crossterm`. CI checks this.

## Replace a capability

1. Keep the contract crate required by consumers.
2. Remove the old implementation dependency from the composition root.
3. Add the replacement implementation crate.
4. Build the workspace.

```toml
[dependencies]
# The old implementation is no longer listed.
my-agent-loop = { path = "../../crates/my-agent-loop" }
```

The replacement must provide the same capability key in the same scope. Once
the old dependency is removed, no overrides or coexistence switch is needed.

Keeping both providers is a composition error. This makes replacement
symmetric with removal and prevents a development-only override from becoming
an untested runtime path.

## Contract boundaries

Contracts exist where consumers must not depend on a concrete implementation:

| Contract | Main interface |
| --- | --- |
| `alpha-host-api` | Module and frontend entry points |
| `alpha-credential-api` | Credential storage |
| `alpha-http-api` | HTTP client and cassette replay |
| `alpha-provider-api` | Providers, adapters, model routes, and catalogs |
| `alpha-prompt-api` | Prompt assembly |
| `alpha-permission-api` | Permission requests, decisions, review, and audit |
| `alpha-agent-api` | Agent driver, session runtime, and provider registry |

An implementation crate can add behavior behind a contract without making
frontends or unrelated modules depend on that implementation.

## Frontend replacement

A frontend crate contributes command metadata and an entry function. The
function receives `FrontRequest` and returns `FrontError` on failure.

The full composition wires `desktop`, `chat`, `tui`, and `run`. The trimmed
composition wires only `run`. Adding a new frontend requires a crate, one entry
function, and metadata under `[package.metadata.alpha]`.

Frontend crates do not depend on core implementation crates such as
`alpha-provider-api`. They use `alpha-front-runtime` and shared schema types.

## CI coverage

Composition tests build temporary Cargo workspaces and verify:

- The dependency graph produces the expected module set.
- Missing requirements fail `cargo check`.
- Duplicate singleton providers fail without modifying source files.
- Registry collisions fail.
- Removing a frontend removes its commands.
- The trimmed app contains no desktop or terminal UI crates.

The generated file is a build artifact. Do not edit or commit it.
