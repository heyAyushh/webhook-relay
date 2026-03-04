# Configuration Reference

## Overview

Configuration is loaded from environment variables (with an optional `.env` file). Start by copying `.env.default`:

```bash
cp .env.default .env
```

The Kafka core can also be configured via `config/kafka-core.toml` for settings that prefer TOML (broker lists, TLS paths, producer/consumer tuning).

---

## Required Variables

These must be set before serve will start.

| Variable | Description |
|---|---|
| `KAFKA_BROKERS` | Comma-separated broker addresses. e.g. `100.64.0.10:9093` |
| `HMAC_SECRET_GITHUB` | HMAC secret for GitHub webhook signatures. Required when `github` is in `RELAY_ENABLED_SOURCES`. |
| `HMAC_SECRET_LINEAR` | HMAC secret for Linear webhook signatures. Required when `linear` is in `RELAY_ENABLED_SOURCES`. |

---

## Kafka

| Variable | Default | Description |
|---|---|---|
| `KAFKA_BROKERS` | — | **Required.** Broker host:port list, comma-separated. |
| `KAFKA_SECURITY_PROTOCOL` | `ssl` | `ssl` or `plaintext`. Plaintext requires explicit opt-in (see below). |
| `KAFKA_ALLOW_PLAINTEXT` | `false` | Must be `true` when `KAFKA_SECURITY_PROTOCOL=plaintext`. Double opt-in guard. |
| `KAFKA_TLS_CERT` | — | Path to client certificate file. Required when protocol is `ssl`. |
| `KAFKA_TLS_KEY` | — | Path to client private key file. Required when protocol is `ssl`. |
| `KAFKA_TLS_CA` | — | Path to CA certificate file. Required when protocol is `ssl`. |
| `KAFKA_DLQ_TOPIC` | `webhooks.dlq` | Topic name for failed delivery messages. |
| `KAFKA_AUTO_CREATE_TOPICS` | `true` | Automatically create topics on startup if they don't exist. |
| `KAFKA_TOPIC_PARTITIONS` | `3` | Partition count for auto-created topics. Must be positive. |
| `KAFKA_TOPIC_REPLICATION_FACTOR` | `1` | Replication factor for auto-created topics. Must be positive. |

### Plaintext opt-in

Plaintext Kafka requires both flags to prevent accidental misconfiguration:

```bash
KAFKA_SECURITY_PROTOCOL=plaintext
KAFKA_ALLOW_PLAINTEXT=true
```

Any other value for `KAFKA_SECURITY_PROTOCOL` is rejected at startup.

---

## Sources

| Variable | Default | Description |
|---|---|---|
| `RELAY_ENABLED_SOURCES` | `github,linear` | Comma-separated list of active webhook sources. Lowercased. Cannot be empty. |
| `RELAY_SOURCE_TOPIC_PREFIX` | `webhooks` | Topic prefix for auto-derived source topics. Topics become `<prefix>.<source>`. Cannot be empty. |
| `RELAY_SOURCE_TOPICS` | _(derived)_ | Override the full source topic list. e.g. `custom.github,custom.linear`. When set, must include a topic for every enabled source. |
| `HMAC_SECRET_GITHUB` | — | Required when `github` is enabled. |
| `HMAC_SECRET_LINEAR` | — | Required when `linear` is enabled. |
| `HMAC_SECRET_EXAMPLE` | — | Required when `example` is enabled. |

Source names are normalised to lowercase ASCII. Custom sources can be added in code (see `add-webhook-source` skill).

---

## Serve / HTTP

| Variable | Default | Description |
|---|---|---|
| `RELAY_BIND` | `0.0.0.0:8080` | TCP address serve listens on. |
| `RELAY_MAX_PAYLOAD_BYTES` | `1048576` (1 MiB) | Maximum accepted request body size. Requests exceeding this are rejected with 413. |
| `RELAY_IP_RATE_PER_MINUTE` | `100` | Maximum requests per minute per client IP. |
| `RELAY_SOURCE_RATE_PER_MINUTE` | `500` | Maximum requests per minute per webhook source. |
| `RELAY_TRUST_PROXY_HEADERS` | `false` | When `true`, `X-Forwarded-For` and `X-Real-IP` are trusted for rate limiting. Requires `RELAY_TRUSTED_PROXY_CIDRS`. |
| `RELAY_TRUSTED_PROXY_CIDRS` | `127.0.0.1/32,::1/128` | Comma-separated CIDR list of trusted upstream proxies. Only used when `RELAY_TRUST_PROXY_HEADERS=true`. |

---

## Deduplication and Cooldown

| Variable | Default | Description |
|---|---|---|
| `RELAY_DEDUP_TTL_SECONDS` | `604800` (7 days) | How long to remember event IDs for deduplication. Must be positive. |
| `RELAY_COOLDOWN_SECONDS` | `30` | Per-entity cooldown window. Events for the same entity within this window are deduplicated at the cooldown level. Must be positive. |

---

## Linear-Specific

| Variable | Default | Description |
|---|---|---|
| `RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW` | `true` | Reject Linear webhooks with a timestamp outside the window. Replay protection. |
| `RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS` | `60` | Maximum age in seconds for a valid Linear webhook timestamp. Must be positive. |

---

## Kafka Publisher (Serve)

| Variable | Default | Description |
|---|---|---|
| `RELAY_PUBLISH_QUEUE_CAPACITY` | `4096` | Internal async publish queue depth. Backpressure is applied when full. |
| `RELAY_PUBLISH_MAX_RETRIES` | `5` | Number of Kafka publish retries before giving up. |
| `RELAY_PUBLISH_BACKOFF_BASE_MS` | `200` | Initial retry backoff in milliseconds. |
| `RELAY_PUBLISH_BACKOFF_MAX_MS` | `5000` | Maximum retry backoff cap in milliseconds. |

---

## Contract and Profile

| Variable | Default | Description |
|---|---|---|
| `RELAY_PROFILE` | `default-openclaw` | Active profile name. Must match a `[profiles.<name>]` key in the contract. |
| `RELAY_CONTRACT_PATH` | — | Explicit path to a `contract.toml` file. Overrides all discovery. |
| `RELAY_VALIDATION_MODE` | `strict` | `strict` or `debug`. Debug mode relaxes non-security checks only. |
| `RELAY_INGRESS_ADAPTER_ID` | — | Override the active ingress adapter ID for serve. |
| `RELAY_INGRESS_ADAPTERS_JSON` | — | JSON array of ingress adapter configs for env-driven adapter setup. |
| `RELAY_SERVE_ROUTES_JSON` | — | JSON array of serve route configs for env-driven route setup. |

---

## Smash / Consumer

Smash reads its configuration from the same Kafka env vars above, plus the contract for adapter-specific settings. Consumer-specific tuning uses the Kafka core TOML config (`config/kafka-core.toml`).

Key smash env vars (from the contract's `token_env` fields):

| Variable | Description |
|---|---|
| `OPENCLAW_WEBHOOK_TOKEN` | Bearer token for `openclaw_http_output` adapter. Referenced via `token_env = "OPENCLAW_WEBHOOK_TOKEN"` in the contract. |

Any env var name can be used as `token_env` — the contract references the variable name, not the value.

---

## Logging

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level filter. Values: `error`, `warn`, `info`, `debug`, `trace`. Supports per-module filtering: `RUST_LOG=webhook_relay=debug,info`. |

---

## `config/kafka-core.toml`

For deployments that prefer TOML for Kafka settings. All fields can also be set via env vars — env takes precedence.

```toml
[kafka_core]
brokers = ["100.64.0.10:9093"]
security_protocol = "ssl"
allow_plaintext = false
topic_prefix_core = "webhooks"
dlq_topic = "webhooks.dlq"
auto_create_topics = true
topic_partitions = 3
topic_replication_factor = 1

[kafka_core.producer_defaults]
publish_queue_capacity = 4096
publish_max_retries = 5
publish_backoff_base_ms = 200
publish_backoff_max_ms = 5000

[kafka_core.consumer_defaults]
commit_mode = "async"
auto_offset_reset = "latest"

[kafka_core.tls]
cert_path = "/etc/relay/certs/relay.crt"
key_path = "/etc/relay/certs/relay.key"
ca_path = "/etc/relay/certs/ca.crt"
```

---

## Boolean Env Var Parsing

All boolean env vars accept: `1`, `true`, `yes`, `on` (case-insensitive) as truthy. Anything else (including unset) is falsy.

---

## Minimal Working `.env` (plaintext, for local dev)

```bash
KAFKA_BROKERS=127.0.0.1:9092
KAFKA_SECURITY_PROTOCOL=plaintext
KAFKA_ALLOW_PLAINTEXT=true

RELAY_ENABLED_SOURCES=github,linear
HMAC_SECRET_GITHUB=dev-secret-github
HMAC_SECRET_LINEAR=dev-secret-linear

OPENCLAW_WEBHOOK_TOKEN=dev-token

RUST_LOG=debug
```

---

## Minimal Working `.env` (TLS, for production)

```bash
KAFKA_BROKERS=100.64.0.10:9093
KAFKA_SECURITY_PROTOCOL=ssl
KAFKA_TLS_CERT=/etc/relay/certs/relay.crt
KAFKA_TLS_KEY=/etc/relay/certs/relay.key
KAFKA_TLS_CA=/etc/relay/certs/ca.crt

RELAY_ENABLED_SOURCES=github,linear
HMAC_SECRET_GITHUB=<github-webhook-secret>
HMAC_SECRET_LINEAR=<linear-webhook-secret>

OPENCLAW_WEBHOOK_TOKEN=<openclaw-token>

RUST_LOG=info
```
