---
name: release-workflow
description: >
  End-to-end release process for hook: version bumps, local validation gate,
  binary release via tag push, and crates.io publish in dependency order. Use
  when cutting a new release or diagnosing a failed publish.
---

# Release Workflow

## What Gets Released

| Artifact | Destination |
|---|---|
| `hook` binary | GitHub Releases |
| `webhook-relay` binary | GitHub Releases |
| `kafka-openclaw-hook` binary | GitHub Releases |
| `relay-core` crate | crates.io |
| `webhook-relay` crate | crates.io |
| `kafka-openclaw-hook` crate | crates.io |

`hook-runtime` is an internal workspace path dependency — not published to crates.io.

---

## Step 1 — Bump Versions

Update `version` in each `Cargo.toml` that is changing. All workspace members must stay aligned:

- `Cargo.toml` (workspace root)
- `crates/relay-core/Cargo.toml`
- `src/Cargo.toml` (webhook-relay)
- `apps/kafka-openclaw-hook/Cargo.toml`
- `tools/hook/Cargo.toml`
- `crates/hook-runtime/Cargo.toml`

If `relay-core` version changes, also update its `version = "..."` in any crate that depends on it.

Commit the bump:

```bash
git add -u
git commit -m "chore: bump version to v0.x.0"
```

---

## Step 2 — Local Validation Gate

All checks must pass before tagging. Do not skip any step.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
scripts/build-release-binaries.sh
scripts/publish-crates.sh --dry-run
```

Common dry-run failures:
- Version not bumped in a dependent crate
- `relay-core` version mismatch in downstream `Cargo.toml` files
- Uncommitted changes in working tree

---

## Step 3 — Tag and Push (Binary Release)

```bash
git tag v0.x.0
git push origin v0.x.0
```

This triggers `.github/workflows/release-binaries.yml`, which builds binaries for linux and macOS and attaches archives + checksums to a GitHub Release.

Expected release assets:
- `webhook-relay-<target>.tar.gz`
- `kafka-openclaw-hook-<target>.tar.gz`
- `hook-<target>.tar.gz`
- `SHA256SUMS-<target>.txt`

Monitor CI:

```bash
gh run list --workflow release-binaries.yml
gh run view <run-id>
```

---

## Step 4 — Publish Crates

Publish order is fixed — `relay-core` must land on crates.io before dependents can be published.

### Option A — GitHub Actions (recommended)

```bash
gh workflow run publish-crates.yml -f dry_run=true   # verify first
gh workflow run publish-crates.yml -f dry_run=false  # then publish
```

### Option B — Local

```bash
scripts/publish-crates.sh
```

Enforced publish order:
1. `relay-core`
2. `webhook-relay`
3. `kafka-openclaw-hook`

For partial publishes, use skip flags:

```bash
scripts/publish-crates.sh --skip-relay-core
```

---

## Step 5 — Verify

```bash
# Allow a few minutes for crates.io index propagation
cargo search relay-core
cargo search webhook-relay
gh release view v0.x.0
```

---

## Rollback

crates.io versions are **immutable** — yank, then cut a patch:

```bash
cargo yank --version 0.x.0 relay-core
cargo yank --version 0.x.0 webhook-relay
cargo yank --version 0.x.0 kafka-openclaw-hook
```

Bump to `0.x.1`, fix the issue, republish. Document in changelog.

For binary releases: delete the GitHub Release and re-tag.

---

## CI Failure Triage

Check in order:
1. `CARGO_REGISTRY_TOKEN` secret configured in repo settings
2. Version already exists on crates.io (bump required)
3. `relay-core` version mismatch in downstream `Cargo.toml`
4. crates.io index propagation delay — wait 2–5 minutes between publishes
5. Reproduce locally with `scripts/publish-crates.sh --dry-run`
