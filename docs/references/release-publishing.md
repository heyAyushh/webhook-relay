# Release and Publishing Runbook

## Scope

This repository currently publishes:

- Binary artifact `webhook-relay` to GitHub Releases.
- Binary artifact `kafka-openclaw-hook` to GitHub Releases.
- Binary artifact `hook` to GitHub Releases.
- Crate `relay-core` to crates.io.
- Crate `webhook-relay` to crates.io.
- Crate `kafka-openclaw-hook` to crates.io.

`hook-runtime` is a workspace crate used as an internal dependency path in this repository and is not currently part of the publish script.

## Prerequisites

- Version bumps committed in relevant `Cargo.toml` files
- CI green on `main`
- `CARGO_REGISTRY_TOKEN` configured for crates publishing

## Version Bump Matrix

When cutting a release, verify version updates in all relevant manifests:

- Workspace crate:
  - `Cargo.toml` (`[package].version` for `webhook-relay`)
- Published library crate:
  - `crates/relay-core/Cargo.toml`
- Published app crate:
  - `apps/kafka-openclaw-hook/Cargo.toml`
- Internal utility binary (release artifact only, not crates publish):
  - `tools/hook/Cargo.toml`

Also verify dependency version alignment where path+version is used (for example `relay-core` version pins in dependent crates).

## Local Validation (Required)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
scripts/build-release-binaries.sh
scripts/publish-crates.sh --dry-run
```

## Binary Release Flow

1. Create and push tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

2. Workflow `.github/workflows/release-binaries.yml` runs.
3. Artifacts and checksums are attached to release.

Expected release assets include:
- `webhook-relay-<target>.tar.gz`
- `kafka-openclaw-hook-<target>.tar.gz`
- `hook-<target>.tar.gz`
- `SHA256SUMS-<target>.txt`

## Crates Publish Flow

1. Run `scripts/publish-crates.sh --dry-run` locally.
2. Trigger GitHub Actions `Publish Crates` workflow:
- first with `dry_run = true`
- then with `dry_run = false`
3. Use script skip flags for partial publishes when needed.

Publish order is enforced:
1. `relay-core`
2. `webhook-relay`
3. `kafka-openclaw-hook`

## CI Publish Failure Checks

If publish fails in CI, check in this order:

1. Auth and permissions
- `CARGO_REGISTRY_TOKEN` exists in repository secrets.
- Workflow permissions allow publishing and reading required contents.

2. Version collisions
- Confirm target versions are not already published on crates.io.
- If already published, bump patch/minor and re-run.

3. Dependency/version mismatch
- Confirm downstream crates reference the exact new `relay-core` version.
- Ensure `Cargo.lock` and manifests are consistent.

4. Publish ordering and propagation
- `relay-core` must publish first.
- If downstream publish fails immediately after `relay-core`, wait briefly and re-run (index propagation delay).

5. Packaging/build failures
- Re-run local gate:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `scripts/publish-crates.sh --dry-run`

6. Workflow config drift
- Validate `.github/workflows/publish-crates.yml` still matches current crate set and publish order.

## Best Practices

- Do not publish from dirty release states.
- Keep dependent crate versions aligned when path+version dependencies change.
- Publish `relay-core` first.
- Keep release artifacts immutable per tag.

## Rollback Notes

- crates.io versions are immutable.
- For bad publishes:
1. yank affected versions
2. cut patch version
3. republish and document changes
