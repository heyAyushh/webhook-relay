# Repository Guidelines

## Project Structure

- `src/`: `webhook-relay` ingress service.
  - `main.rs`: Axum server, `/webhook/{source}`, health/readiness, rate limiting.
  - `producer.rs`: Kafka producer with retry/backoff worker.
  - `sources/`: source-specific auth + event extraction (`github`, `linear`).
- `crates/relay-core/`: shared models/signatures/sanitization logic.
- `deploy/nginx/`: TLS termination config.
- `systemd/`: runtime unit files.
- `scripts/gen-certs.sh`: mTLS bootstrap helper.
- `memory/coder-tasks.md`: orchestrator shared state board.

## Build, Test, and Dev Commands

- Format: `cargo fmt --all`
- Lint (if installed): `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace`
- Release build: `cargo build --workspace --release`
- Generate mTLS certs: `scripts/gen-certs.sh`
- Bootstrap full local setup: `scripts/init.sh --up`
- Start relay stack: `docker compose -f docker-compose.yml up --build`
- Start relay dev override stack: `docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build`

## Coding Standards

- Rust-first codebase; prefer explicit types on boundary structs/config.
- No magic numbers: define constants with clear names.
- Keep functions single-purpose and small; extract helpers early.
- Fail closed on auth/validation paths.
- Never log webhook payload bodies on auth failures.
- Keep source-specific security logic in `src/sources/*`.

## Testing Expectations

- Add/adjust unit tests when changing:
  - signature validation
  - event-type extraction
  - retry/backoff behavior
  - envelope schema
- Minimum pre-PR checks:
  - `cargo fmt --all`
  - `cargo test --workspace`
  - `cargo build --workspace --release`

## Commit and PR Guidelines

- Use Conventional Commits.
- Keep commits scoped by component (`relay`, `consumer`, `docs`, `ops`).
- PRs should include:
  - behavior summary
  - exact test commands run
  - config/deploy impact (`.env`, compose, systemd, TLS)

## Notes

- Legacy shell/Python relay scripts have been removed from the active runtime path.
- If a requested skill like `$init` is not available in session skills, proceed with the closest manual equivalent and document the outcome.
