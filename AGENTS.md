# Repository Guidelines

## Project Structure & Module Organization
- `hooks.yaml`: webhook endpoint definitions (`github-pr`, `linear`) and env mapping from inbound requests.
- `scripts/`: operational code.
  - `relay-github.sh`, `relay-linear.sh`: verify, deduplicate, cooldown, and forward events.
  - `sanitize-payload.py`: sanitizes untrusted webhook text before forwarding to OpenClaw.
  - `smoke-test.sh`: end-to-end validation with a local mock OpenClaw service.
  - `commit-and-push.sh`: helper for committing/pushing from non-protected branches.
- `references/`: runbooks and integration docs (GitHub, Linear, Tailscale, setup flow).
- `SKILL.md`: high-level capabilities and usage context for this repository.

## Build, Test, and Development Commands
- Install required webhook server: `brew install webhook`  
  Alternative: `go install github.com/adnanh/webhook@latest`.
- Run locally: `webhook -hooks hooks.yaml -verbose -port 9000`.
- Quick sanitizer check:  
  `echo '{"action":"opened"}' | python3 scripts/sanitize-payload.py --source github`.
- Run smoke test with local mock OpenClaw:  
  `GITHUB_WEBHOOK_SECRET=... LINEAR_WEBHOOK_SECRET=... OPENCLAW_HOOKS_TOKEN=... scripts/smoke-test.sh`.
- Run smoke test against live OpenClaw: add `OPENCLAW_GATEWAY_URL=...` and pass `-l`.

## Coding Style & Naming Conventions
- Bash: start with `#!/usr/bin/env bash`, `set -euo pipefail`, and `IFS=$'\n\t'`.
- Use named `readonly` constants (no magic numbers) and descriptive `snake_case` function names.
- Keep functions focused on one responsibility (`require_env`, `verify_linear_signature`, etc.).
- Python (`scripts/sanitize-payload.py`): follow PEP 8, keep type hints, and prefer explicit helper names.
- Comments should explain intent/risk tradeoffs (especially security logic), not obvious steps.

## Testing Guidelines
- Primary validation is `scripts/smoke-test.sh` (signature handling, dedup, cooldown, forwarding).
- Before opening a PR, run at least:
  - `bash -n scripts/*.sh`
  - `python3 -m py_compile scripts/sanitize-payload.py`
  - the smoke test command above.
- Update smoke scenarios when changing hook rules, signature verification, or payload sanitization behavior.

## Commit & Pull Request Guidelines
- Use Conventional Commits; current history follows prefixes like `feat:`, `docs:`, and `chore:`.
- Write imperative, meaningful summaries (example: `feat: add webhook relay and smoke test scripts`).
- PRs should include:
  - a short summary of behavior changes,
  - a test plan with exact commands executed,
  - linked issue/context when applicable,
  - payload or log snippets for relay-path changes.
