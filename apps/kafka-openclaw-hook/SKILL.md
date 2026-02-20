---
name: kafka-openclaw-hook
description: >
  Maintain and extend the kafka-openclaw-hook consumer app that reads webhook
  envelopes from Kafka and forwards to OpenClaw with retry and DLQ fallback.
  Use when editing consumer loop behavior, retry policy, OpenClaw payload shape,
  DLQ logic, or related configuration and tests.
---

# kafka-openclaw-hook Skill

## Scope

- `src/main.rs`: wiring and startup
- `src/config.rs`: env-driven config validation
- `src/consumer.rs`: consume loop and offset commits
- `src/forwarder.rs`: OpenClaw POST contract + retry/backoff
- `src/dlq.rs`: DLQ publish path

## Guardrails

- Keep consume path idempotent and safe under re-delivery.
- Preserve at-least-once semantics; do not acknowledge early.
- Keep retry classification explicit:
  - retry on timeout/connect/request errors and `429`/`5xx`
  - fail permanent on other `4xx`
- Never include sensitive tokens in logs.

## Change Workflow

1. Update code in the smallest module owning the behavior.
2. Keep OpenClaw payload schema stable unless explicitly changing contract.
3. Add/update tests near changed module.
4. Run:
   - `cargo fmt --all`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test -p kafka-openclaw-hook`

## References

- App docs: `apps/kafka-openclaw-hook/README.md`
- Shared envelope/types: `crates/relay-core/src/model.rs`
- Runtime config example: `.env.default`
