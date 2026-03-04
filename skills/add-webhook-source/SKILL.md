---
name: add-webhook-source
description: >
  Add a new webhook source to hook serve. Covers implementing the SourceHandler
  trait, registering the handler, adding HMAC secret config, creating the Kafka
  topic, and writing tests. Use when integrating a new webhook provider (e.g.
  Stripe, PagerDuty, Bitbucket).
---

# Add Webhook Source

## What a Source Handler Does

A source handler plugs into serve at `POST /webhook/{source}`. It is responsible for:
1. **Signature validation** — authenticate the request (fail closed on missing secret)
2. **Event type extraction** — derive a string like `pull_request.opened`
3. **Dedup key** — a stable unique string per event delivery
4. **Cooldown key** — optional; rate-limits repeated events for the same entity

The result is wrapped into a normalised envelope and published to `webhooks.<source>`.

---

## Step 1 — Implement the Handler

Create `src/sources/<name>.rs`. Model it after `src/sources/github.rs`:

```rust
use crate::config::Config;
use crate::sources::{SourceHandler, ValidationError, header_value, payload_token};
use axum::http::HeaderMap;
use relay_core::signatures::verify_github_signature; // or your own verify fn
use serde_json::Value;

const SOURCE_NAME: &str = "myservice";
const SIGNATURE_HEADER: &str = "X-MyService-Signature";
const EVENT_HEADER: &str = "X-MyService-Event";
const MISSING_SECRET_MESSAGE: &str = "missing myservice secret";

#[derive(Debug, Default)]
pub struct MyServiceHandler;

pub static HANDLER: MyServiceHandler = MyServiceHandler;

impl SourceHandler for MyServiceHandler {
    fn source_name(&self) -> &'static str {
        SOURCE_NAME
    }

    fn validate_request(
        &self,
        config: &Config,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ValidationError> {
        let secret = config
            .hmac_secret_myservice          // add this field to Config (Step 3)
            .as_deref()
            .ok_or(ValidationError::Unauthorized(MISSING_SECRET_MESSAGE))?;
        let signature = header_value(headers, SIGNATURE_HEADER)
            .ok_or(ValidationError::Unauthorized("missing signature"))?;
        if verify_signature(secret, body, &signature) {
            Ok(())
        } else {
            Err(ValidationError::Unauthorized("invalid signature"))
        }
    }

    fn event_type(&self, headers: &HeaderMap, _payload: &Value) -> Result<String, ValidationError> {
        header_value(headers, EVENT_HEADER)
            .ok_or(ValidationError::BadRequest("missing event header"))
    }

    fn dedup_key(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        // Build a stable unique key — use a delivery ID header if available,
        // otherwise fall back to a payload field.
        let delivery_id = header_value(headers, "X-MyService-Delivery")
            .or_else(|| payload_token(payload, &["id"]))
            .ok_or(ValidationError::BadRequest("cannot derive dedup key"))?;
        Ok(format!("myservice:{delivery_id}"))
    }

    fn cooldown_key(&self, payload: &Value) -> Option<String> {
        // Return None if cooldown is not applicable for this source.
        let entity_id = payload_token(payload, &["entity", "id"])?;
        Some(format!("cooldown-myservice-{entity_id}"))
    }
}

fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    // Implement HMAC-SHA256 verification matching the provider's scheme.
    // relay_core::signatures has helpers for GitHub-style sha256= prefixed sigs.
    use relay_core::signatures::verify_github_signature;
    verify_github_signature(secret, body, signature)
}
```

Key rules:
- `validate_request` must **fail closed** — return `Unauthorized` if the secret is missing, never skip validation.
- Never log the raw body or secret on auth failures.
- `dedup_key` must be stable for the same logical delivery (idempotency depends on it).

---

## Step 2 — Register the Handler

Edit `src/sources/mod.rs`:

```rust
pub mod myservice;   // add this line alongside github, linear

// In SOURCE_HANDLERS LazyLock:
handlers.insert(myservice::HANDLER.source_name(), &myservice::HANDLER);
```

The source name (returned by `source_name()`) becomes the URL path segment and Kafka topic suffix. It is normalised to lowercase ASCII.

---

## Step 3 — Add HMAC Secret to Config

Edit `src/config.rs`. Find the existing `hmac_secret_github` / `hmac_secret_linear` fields and add:

```rust
pub hmac_secret_myservice: Option<String>,
```

Wire it up from env in the same pattern as the existing fields (e.g. `HMAC_SECRET_MYSERVICE`).

---

## Step 4 — Create the Kafka Topic

Before running serve, create the source topic (see `kafka-topic-setup` skill):

```bash
SOURCES="github linear myservice" \
  skills/kafka-topic-setup/scripts/create-hook-topics.sh
```

Or manually:

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.myservice \
  --partitions 3 \
  --replication-factor 1 \
  --config retention.ms=604800000
```

---

## Step 5 — Add Relay Topic Arg

Add the new topic to the relay invocation:

```bash
hook relay \
  --topics webhooks.github,webhooks.linear,webhooks.myservice \
  --output-topic webhooks.core
```

---

## Step 6 — Set the Secret in `.env`

```bash
HMAC_SECRET_MYSERVICE=your-secret-here
```

---

## Step 7 — Write Tests

Add a test module in `src/sources/myservice.rs` following the pattern in `github.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use relay_core::signatures::compute_hmac_sha256_hex;
    use serde_json::json;

    #[test]
    fn validates_correct_signature() {
        let secret = "test-secret";
        let body = br#"{"id":1}"#;
        let digest = compute_hmac_sha256_hex(secret, body);
        let mut headers = HeaderMap::new();
        headers.insert(
            SIGNATURE_HEADER,
            HeaderValue::from_str(&format!("sha256={digest}")).unwrap(),
        );
        let config = /* build a minimal config with hmac_secret_myservice set */;
        assert!(HANDLER.validate_request(&config, &headers, body).is_ok());
    }

    #[test]
    fn rejects_missing_secret_in_config() {
        let headers = HeaderMap::new();
        let config = /* config with hmac_secret_myservice = None */;
        let result = HANDLER.validate_request(&config, &headers, b"body");
        assert!(matches!(result, Err(ValidationError::Unauthorized(_))));
    }

    #[test]
    fn extracts_event_type_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert(EVENT_HEADER, HeaderValue::from_static("issue.created"));
        let result = HANDLER.event_type(&headers, &json!({}));
        assert_eq!(result.unwrap(), "issue.created");
    }
}
```

Run tests:

```bash
cargo test -p webhook-relay sources::myservice
```

---

## Checklist

- [ ] `src/sources/myservice.rs` created with `SourceHandler` impl
- [ ] Registered in `src/sources/mod.rs` `SOURCE_HANDLERS`
- [ ] `hmac_secret_myservice` field added to `Config` and read from env
- [ ] `webhooks.myservice` Kafka topic created
- [ ] `--topics` arg on relay updated
- [ ] `HMAC_SECRET_MYSERVICE` set in `.env`
- [ ] Unit tests added for signature validation, event type, dedup key
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
