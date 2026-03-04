# Repository Guidelines

## Project Structure

- `src/`: `webhook-relay` serve runtime (Axum, `/webhook/{source}`, health/readiness, rate limiting).
- `tools/hook/`: CLI/operator control plane (`hook serve`, `hook relay`, `hook smash`, ops commands).
- `apps/default-openclaw/`: canonical compatibility contract (`contract.toml`), default flow `http_webhook_ingress → kafka core → openclaw_http_output`.
- `apps/kafka-openclaw-hook/`: compatibility binary wrapper that calls `hook_runtime::smash::run_from_env()`.
- `crates/hook-runtime/`: runtime execution engine (adapters + smash runtime).
- `crates/relay-core/`: shared contracts, validator, model, signatures, sanitization.
- `config/`: Kafka-core defaults and schema examples.
- `docs/`: changelog, spec, roadmap, and references.
- `firecracker/`: Firecracker microVM artifacts for binary-first deployment.
  - `runtime/`: portable jailer launcher, cleanup, overwatcher, broker inventory helpers.
  - `systemd/`: host service templates (`firecracker@.service`, network, proxy-mux, watchdog timer, external checker units) and env examples.
  - `watchdog/`: local watchdog (auto-recovery + heartbeat), boot/shutdown loggers, alert helper, and external blackbox/chisel checker scripts.
- `skills/`: operational skills and runbooks.
  - `kafka-kraft-firecracker/`: deploy single-node Kafka KRaft in a Firecracker VM.

## Build, Test, and Dev Commands

- Format: `cargo fmt --all`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace`
- Release build: `cargo build --workspace --release`
- Build release archives: `scripts/build-release-binaries.sh`
- Crates publish dry-run: `scripts/publish-crates.sh --dry-run`
- Generate mTLS certs: `scripts/gen-certs.sh`
- Firecracker host network setup: `sudo scripts/setup-firecracker-bridge-network.sh`
- Firecracker host network teardown: `sudo scripts/teardown-firecracker-bridge-network.sh`
- Shell syntax check: `bash -n <script>`

## CLI Quick Reference

```bash
cargo install --path tools/hook

hook serve --app default-openclaw
hook relay --topics webhooks.github,webhooks.linear --output-topic webhooks.core
hook smash --app default-openclaw
hook debug capabilities
```

## Coding Standards

- Rust-first codebase; prefer explicit types on boundary structs/config.
- No magic numbers: define constants with clear names.
- Keep functions single-purpose and small; extract helpers early.
- Fail closed on auth/validation paths.
- Never log webhook payload bodies on auth failures.
- Unsupported contract drivers are rejected only when active in the selected profile.

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
- Keep commits scoped by component (`serve`, `relay`, `smash`, `docs`, `ops`).
- PRs should include:
  - behavior summary
  - exact test commands run
  - config/deploy impact (`.env`, contract, systemd, TLS)
