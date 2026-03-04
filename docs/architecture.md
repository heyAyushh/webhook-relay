# Architecture

## Overview

hook is a contract-driven event workbench built around three runtime roles and a mandatory Kafka backbone. Every event that enters the system travels the same path regardless of how it arrived or where it is going:

```
External Source
      │
      ▼
  ┌────────┐    webhooks.<source>    ┌───────┐    webhooks.core    ┌───────┐
  │ serve  │ ──────────────────────▶ │ relay │ ──────────────────▶ │ smash │ ──▶ Destination
  └────────┘                         └───────┘                      └───────┘
                                                                         │
                                                                         ▼
                                                                    webhooks.dlq
```

Kafka is mandatory between serve and smash in every profile. There is no path that bypasses it.

---

## Runtime Roles

### serve (`src/`, `webhook-relay`)

The ingress layer. Receives raw events from external sources, authenticates them, normalises them into a typed envelope, and publishes to an internal Kafka source topic.

Responsibilities:
- Run one or more ingress adapters (HTTP, WebSocket, MCP push, external Kafka)
- Validate request signatures (HMAC-SHA256, constant-time comparison)
- Parse and sanitize the raw payload
- Assign a UUID event ID and RFC3339 timestamp
- Derive the event type string from headers and payload
- Check deduplication (in-memory TTL store)
- Apply cooldown rate limiting per entity
- Execute serve-side plugins (event_type_alias, require_payload_field, add_meta_flag)
- Publish the `EventEnvelope` to the appropriate Kafka source topic
- Apply IP-level and per-source rate limiting

serve never writes directly to `webhooks.core`. It always publishes to a per-source topic (e.g. `webhooks.github`) that relay then consumes.

### relay (`tools/hook/src/commands/relay.rs`)

The internal bridge. Consumes from all active serve-side source topics and republishes to the single internal core topic (`webhooks.core`). No transformation is applied — relay is a pure fan-in.

Responsibilities:
- Consume from one or more `webhooks.<source>` topics
- Republish each message verbatim to `webhooks.core`
- Maintain its own consumer group offset

relay has no contract. It is configured entirely via CLI flags:

```bash
hook relay \
  --topics webhooks.github,webhooks.linear \
  --output-topic webhooks.core
```

### smash (`crates/hook-runtime/`)

The egress layer. Consumes from `webhooks.core`, resolves the active smash route, and delivers to one or more configured destinations.

Responsibilities:
- Consume from `webhooks.core` (or a configured core topic pattern)
- Match the route against the message topic and event type
- Execute smash-side plugins per destination adapter
- Deliver to required destinations (commit blocked until all succeed)
- Deliver to optional destinations (failures never block commit)
- Retry on retryable failures with per-adapter backoff
- Publish to `webhooks.dlq` when required delivery is exhausted

---

## Kafka Topology

```
webhooks.github   ─┐
webhooks.linear   ─┤  (relay fan-in)  ──▶  webhooks.core  ──▶  smash
webhooks.<source> ─┘                                       ──▶  webhooks.dlq
```

| Topic | Producer | Consumer | Purpose |
|---|---|---|---|
| `webhooks.<source>` | serve | relay | Per-source event stream |
| `webhooks.core` | relay | smash | Normalised core stream |
| `webhooks.dlq` | smash | ops | Failed delivery events |

All topics must exist before serve, relay, or smash start. See the `kafka-topic-setup` skill.

---

## Contract System

serve and smash behavior is fully defined by `contract.toml`. The contract declares:

- **Ingress adapters** — what serve accepts
- **Egress adapters** — where smash delivers
- **Routes** — how events flow between adapters and topics
- **Profiles** — which combination of adapters and routes is active at runtime
- **Plugins** — per-adapter transformation/filter steps

```
contract.toml
  [app]        — metadata
  [policies]   — allow_no_output, validation_mode
  [serve]      — ingress_adapters + routes
  [smash]      — egress_adapters + routes
  [profiles.*] — active adapter/route sets
  [transports.*] — outbound MCP transport config
```

The contract validator runs at startup. Any validation error that is security-critical blocks the process from starting. Unknown keys anywhere in the contract are hard errors (`deny_unknown_fields`).

Contract discovery order (first match wins):
1. `--contract <path>`
2. `--app <id>` → `apps/<id>/contract.toml`
3. `./contract.toml`
4. Embedded `default-openclaw` fallback

relay never loads a contract.

---

## Serve Processing Pipeline

For each inbound event, serve executes this pipeline in order:

1. **Ingest** — adapter receives the raw request (HTTP POST, WebSocket frame, MCP tool call, or Kafka message)
2. **Source normalisation** — source name extracted from path or payload, lowercased
3. **Auth** — source handler validates HMAC signature (fails closed if secret is missing or invalid)
4. **Payload parse** — JSON body parsed; non-JSON bodies rejected
5. **Payload validation** — source-specific field checks (e.g. timestamp window for Linear)
6. **Event type extraction** — derived from request headers and payload fields
7. **Dedup check** — event ID looked up in in-memory TTL store; duplicates dropped
8. **Cooldown check** — entity-level rate limiting applied
9. **Sanitization** — payload run through zero-trust sanitizer (relay-core)
10. **Plugin execution** — serve adapter plugins run in declaration order
11. **Envelope creation** — `EventEnvelope` assembled with UUID, timestamp, source, event_type, payload, meta
12. **Publish** — envelope published to `webhooks.<source>` via async Kafka producer
13. **Response** — HTTP 200/202 returned to caller

Steps 3–4 are security-critical: failures always return 401 or 400, never 500.

---

## Smash Processing Pipeline

For each consumed envelope from `webhooks.core`:

1. **Consume** — message read from Kafka
2. **Route match** — smash route resolved by topic pattern and optional event type filters
3. **Plugin execution** — smash adapter plugins run per destination
4. **Required deliveries** — adapters with `required = true` attempted; commit blocked until all succeed
5. **Optional deliveries** — adapters with `required = false` attempted; failures logged but never block commit
6. **Commit** — Kafka offset committed after all required deliveries succeed
7. **Retry** — retryable failures retried with adapter-configured backoff
8. **DLQ** — envelope published to `webhooks.dlq` when retries are exhausted

---

## Adapter Symmetry

Serve ingress and smash egress adapters are symmetric pairs:

| Ingress (serve) | Egress (smash) |
|---|---|
| `http_webhook_ingress` | `openclaw_http_output` |
| `websocket_ingress` | `websocket_client_output` / `websocket_server_output` |
| `mcp_ingest_exposed` | `mcp_tool_output` |
| `kafka_ingress` | `kafka_output` |

Multiple adapters can be active simultaneously on either side in any combination.

---

## Component Map

```
webhook-relay/
├── src/                       # serve runtime (webhook-relay binary)
│   ├── main.rs                # Axum router, request handlers, Kafka publisher
│   ├── config.rs              # env-driven config, all serve settings
│   ├── envelope.rs            # EventEnvelope construction
│   ├── producer.rs            # async Kafka publish queue + worker
│   ├── idempotency.rs         # in-memory dedup store with TTL
│   ├── middleware.rs          # rate limiting middleware
│   ├── client_ip.rs           # IP extraction (direct + trusted proxy)
│   └── sources/               # per-source auth + event extraction
│       ├── mod.rs             # SourceHandler trait + registry
│       ├── github.rs          # GitHub HMAC-SHA256 + event type
│       └── linear.rs          # Linear HMAC-SHA256 + timestamp window
│
├── tools/hook/                # hook CLI (hook binary)
│   └── src/commands/
│       ├── serve.rs           # hook serve — loads contract, runs serve runtime
│       ├── relay.rs           # hook relay — Kafka fan-in bridge
│       └── smash.rs           # hook smash — loads contract, runs smash runtime
│
├── crates/relay-core/         # shared primitives (library crate)
│   ├── contract.rs            # AppContract schema (deny_unknown_fields)
│   ├── contract_validator.rs  # profile validation, fail-closed checks
│   ├── model.rs               # EventEnvelope, EventMeta, DlqEnvelope
│   ├── signatures.rs          # HMAC-SHA256 verify + constant-time compare
│   ├── sanitize.rs            # zero-trust payload sanitizer
│   ├── timestamps.rs          # timestamp window validation
│   ├── keys.rs                # dedup key + cooldown key helpers
│   └── kafka_config.rs        # shared Kafka config from TOML/env
│
├── crates/hook-runtime/       # smash execution engine (library crate)
│   ├── adapters/egress/       # one file per egress adapter driver
│   └── smash/                 # consumer loop, route dispatch, DLQ
│
└── apps/
    ├── default-openclaw/      # canonical contract.toml
    └── kafka-openclaw-hook/   # compatibility binary wrapping smash runtime
```

---

## Default Profile: `default-openclaw`

The built-in profile that every fresh installation uses:

```
HTTP POST /webhook/{source}
        │
        ▼ (serve: http_webhook_ingress)
webhooks.github / webhooks.linear
        │
        ▼ (relay)
webhooks.core
        │
        ▼ (smash: openclaw_http_output)
POST http://127.0.0.1:18789/hooks/agent
```

All active profiles still route through Kafka core. The default profile can be extended without code changes by editing `apps/default-openclaw/contract.toml`.

---

## Security Boundaries

- **Serve boundary**: All inbound requests must pass HMAC-SHA256 validation before any processing. Missing secrets fail closed with 401. Raw unauthorized payloads are never logged.
- **Kafka boundary**: mTLS by default. Plaintext requires two explicit opt-in flags.
- **Smash boundary**: Token for the destination is read from a named env var (never hardcoded in the contract). Required destinations gate offset commit.
- **Sanitizer boundary**: All payloads pass through `relay_core::sanitize` before envelope creation. The sanitizer is configurable but its results are never skipped.

See [security.md](security.md) for the full security model.
