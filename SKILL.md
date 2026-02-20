---
name: webhook-relay
description: >
  Build, maintain, and operate the Rust webhook relay workspace that bridges
  GitHub/Linear webhooks to AutoMQ and forwards to OpenClaw via kafka-openclaw-hook.
  Use when editing ingress auth/validation, rate limits, dedup/cooldown logic,
  sanitizer behavior, Kafka publish/consume flow, OpenClaw forwarding payloads,
  or systemd-based deployment docs for this repo.
---

# Webhook Relay Workspace Skill

## Workspace Map

- `src/`: `webhook-relay` ingress service (`POST /webhook/{source}`, publish to Kafka)
- `apps/kafka-openclaw-hook/`: outbound-only consumer from Kafka to OpenClaw `/hooks/agent`
- `crates/relay-core/`: shared models, signature verification, timestamp checks, sanitizer, key helpers
- `systemd/`: production unit files for binary-first deployment

## Use This Skill To

- add or change webhook-source auth and event-type parsing
- tune ingress rate limits, dedup, cooldown, and replay-window checks
- modify queue/worker publish retry behavior
- adjust consumer retry and DLQ behavior
- maintain compatibility with GitHub and Linear webhook payloads

## Safety Invariants

- verify signatures on raw body bytes before JSON parsing
- never log full untrusted webhook payloads on auth failures
- keep unknown source paths returning `404`
- commit consumer offsets only after forward attempt and DLQ fallback path
- treat sanitize logic as zero-trust boundary and preserve injection flags

## Fast Workflow

1. Edit source-specific logic in `src/sources/` or `crates/relay-core/`.
2. Keep envelope contracts in `crates/relay-core/src/model.rs` backward compatible.
3. Update docs in root `README.md` and crate-level READMEs when behavior changes.
4. Run:
   - `cargo fmt --all`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test --workspace`
   - `cargo build --workspace --release`

## Component Docs

- `README.md` (workspace architecture and ops)
- `apps/kafka-openclaw-hook/README.md` and `apps/kafka-openclaw-hook/SKILL.md`
- `crates/relay-core/README.md` and `crates/relay-core/SKILL.md`
