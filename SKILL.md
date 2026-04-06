---
name: hook-serve
description: "Configure Kafka topics, validate hook contracts, register ingress/egress adapters, and manage serve/relay/smash runtime roles in the contract-driven hook workspace. Use when adding a webhook source, writing or debugging a contract.toml, implementing an adapter or plugin, changing event routing, or updating deployment and operator documentation."
---

# Hook Serve Workspace Skill

## Workspace Map

- `src/`: serve runtime (`hook-serve`) — HTTP/WebSocket/MCP ingress, rate limiting, health checks
- `tools/hook/`: operator CLI (`hook serve`, `hook relay`, `hook smash`, `hook debug`)
- `apps/default-openclaw/`: canonical compatibility contract (`contract.toml`)
- `apps/kafka-openclaw-hook/`: compatibility wrapper binary for smash runtime
- `crates/relay-core/`: contracts, validator, shared envelope/security primitives
- `crates/hook-runtime/`: smash runtime and adapter execution engine
- `config/kafka-core.toml`: Kafka core config reference
- `systemd/`, `firecracker/`, `scripts/`: deployment and operations

## Use This Skill To

- evolve contract schema and profile semantics
- implement or validate serve ingress adapter behavior
- implement or validate smash egress adapter behavior
- add or change plugin execution semantics on either side
- tune fail-closed validation and runtime safety defaults
- update operator docs and runbooks after behavioral changes

## Safety Invariants

- strict fail-closed validation unless debug mode is explicit
- unsupported drivers rejected only when active in selected profile
- Kafka remains mandatory transport between serve and smash
- do not log sensitive secrets/tokens
- preserve required-destination delivery semantics for smash

## Fast Workflow

1. Make the smallest change in the owning module.
2. Update contract/runtime docs when behavior changes.
3. Validate:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If clippy fails on contract changes, verify envelope schema compatibility in `crates/relay-core/`. If tests fail, check that adapter driver enums match the contract schema before re-running.

## Key Docs

- `README.md` — project overview, CLI quick reference, coding standards
- `docs/CHANGELOG.md` — release history and breaking changes
- `docs/spec.md` — contract schema definitions and profile semantics
- `tools/hook/README.md` — CLI subcommands and flags
- `crates/relay-core/README.md` — shared contracts, validation, envelope models
- `crates/hook-runtime/README.md` — smash runtime, adapter execution engine
