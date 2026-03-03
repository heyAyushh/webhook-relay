# relay-core

Shared Rust library for webhook relay services.

## Purpose

`relay-core` contains source-agnostic primitives reused by both runtime apps:

- `model.rs`: source helpers and envelope schemas (`WebhookEnvelope`, `DlqEnvelope`)
- `signatures.rs`: constant-time signature/token verification helpers
- `timestamps.rs`: Linear replay-window extraction/validation
- `sanitize.rs`: zero-trust payload sanitizer with injection pattern detection
- `keys.rs`: dedup/cooldown key-shape helpers

## Design Principles

- Fail closed on malformed auth/timestamp inputs.
- Keep signature checks constant-time for equal-length comparisons.
- Keep sanitizer behavior explicit and test-backed.
- Preserve key formats for parity with existing dedup/cooldown behavior.

## Usage

Example:

```rust
use relay_core::model::{DEFAULT_SOURCE_TOPIC_PREFIX, source_topic_name};
use relay_core::signatures::verify_github_signature;
use relay_core::sanitize::sanitize_payload;

let _topic = source_topic_name(DEFAULT_SOURCE_TOPIC_PREFIX, "github");
let _verified = verify_github_signature("secret", br#"{}"#, "sha256=...");
let _sanitized = sanitize_payload("github", &serde_json::json!({})).unwrap();
```

## Test

From workspace root:

```bash
cargo test -p relay-core
```
