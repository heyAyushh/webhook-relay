---
name: kafka-openclaw-hook
description: "Maintain the kafka-openclaw-hook compatibility binary, update startup configuration, modify smash adapter bindings, and troubleshoot deployment compatibility with hook-runtime smash execution. Use when changing startup wiring, runtime env expectations, deployment compatibility, smash adapter/plugin behavior, or debugging runtime integration issues."
---

# kafka-openclaw-hook Skill

## Scope

- `apps/kafka-openclaw-hook/src/main.rs`: compatibility entrypoint — calls `hook_runtime::smash::run_from_env()`
- `crates/hook-runtime/src/smash/*`: smash runtime behavior
- `crates/hook-runtime/src/adapters/egress/*`: egress adapter drivers

## Guardrails

- keep compatibility process name and startup path stable
- maintain at-least-once delivery semantics for required destinations
- preserve DLQ behavior for required delivery failures
- keep secrets/token values out of logs

## Change Workflow

1. Edit only the owning runtime module.
2. Keep adapter/plugin behavior explicit and test-backed.
3. Update docs if env keys or driver semantics change.
4. Validate:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p hook-runtime
cargo test -p kafka-openclaw-hook
```

If clippy fails, fix warnings before proceeding. If tests fail, check DLQ behavior and at-least-once delivery semantics before re-running — a failing delivery test often means a required destination is misconfigured in the contract.

## References

- `apps/kafka-openclaw-hook/README.md` — binary overview and env expectations
- `crates/hook-runtime/README.md` — smash runtime and adapter engine
- `apps/default-openclaw/contract.toml` — canonical contract example
