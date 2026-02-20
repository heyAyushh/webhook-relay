---
name: relay-core
description: >
  Maintain the shared relay-core crate for signatures, sanitization, timestamp
  verification, key generation, and envelope models used across webhook-relay
  and kafka-openclaw-hook. Use when changing shared contracts or security-critical
  validation logic with parity requirements.
---

# relay-core Skill

## Scope

- `src/model.rs`: shared data contracts and source/topic helpers
- `src/signatures.rs`: HMAC/token verification utilities
- `src/timestamps.rs`: Linear replay-window guard
- `src/sanitize.rs`: payload allowlisting, fencing, and injection flagging
- `src/keys.rs`: dedup/cooldown key naming conventions

## Guardrails

- Maintain backward compatibility for serialized envelope fields.
- Keep signature compare logic constant-time and deterministic.
- Preserve sanitizer allowlists and flags unless explicitly changing policy.
- Treat unknown source strings as explicit errors, not silent fallback.

## Change Workflow

1. Update only the module that owns the behavior.
2. Add or adjust unit tests in the same module.
3. If behavior changes, update dependent app docs (`README.md` files).
4. Run:
   - `cargo fmt --all`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo test -p relay-core`

## References

- Workspace docs: `README.md`
- Relay ingress app: `src/`
- Consumer app: `apps/kafka-openclaw-hook/src/`
