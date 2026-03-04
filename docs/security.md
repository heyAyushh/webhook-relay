# Security

## Principles

1. **Fail closed** — missing auth material always results in rejection, never a pass-through.
2. **No raw payload logging** — unauthorized or malformed payloads are never written to logs.
3. **Explicit plaintext opt-in** — Kafka plaintext requires two separate env flags.
4. **Secrets referenced by name** — contract files contain env var names, never secret values.
5. **Constant-time comparison** — all HMAC and token comparisons use constant-time functions.

---

## Webhook Signature Validation

Every inbound webhook is validated before the payload is processed.

### GitHub (HMAC-SHA256)

GitHub sends a `X-Hub-Signature-256: sha256=<hex>` header. Serve:
1. Reads `HMAC_SECRET_GITHUB` from env (required — fails closed if missing).
2. Computes HMAC-SHA256 over the raw request body using the secret.
3. Compares the computed digest to the header value using constant-time comparison.
4. Returns 401 if missing, 401 if invalid.

The secret is never logged, never stored in the contract, and never exposed in health endpoints.

### Linear (HMAC-SHA256 + timestamp window)

Linear sends a `Linear-Signature: <hex>` header and includes a timestamp in the payload.

Serve validates:
1. HMAC-SHA256 signature using `HMAC_SECRET_LINEAR` (required — fails closed if missing).
2. Timestamp window: the payload timestamp must be within `RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS` (default: 60s) of the current time. This prevents replay attacks.

Timestamp validation is enabled by default (`RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW=true`) and can only be disabled with an explicit opt-out.

### Example source

The example source (`HMAC_SECRET_EXAMPLE`) follows the same HMAC-SHA256 pattern as GitHub. It exists for testing only and should never be enabled in production.

---

## Kafka Transport Security

### TLS (default)

```bash
KAFKA_SECURITY_PROTOCOL=ssl
KAFKA_TLS_CERT=/etc/relay/certs/relay.crt
KAFKA_TLS_KEY=/etc/relay/certs/relay.key
KAFKA_TLS_CA=/etc/relay/certs/ca.crt
```

All three TLS files are required when `ssl` is the protocol. Missing any of them is a startup error.

Generate self-signed mTLS certificates for local or private deployments:

```bash
scripts/gen-certs.sh
```

This creates a CA certificate, a relay certificate, and a consumer certificate. Place them at the paths referenced in your env.

### Plaintext opt-in

Plaintext Kafka requires explicit double opt-in to prevent accidental misconfiguration:

```bash
KAFKA_SECURITY_PROTOCOL=plaintext
KAFKA_ALLOW_PLAINTEXT=true
```

If only `KAFKA_SECURITY_PROTOCOL=plaintext` is set without `KAFKA_ALLOW_PLAINTEXT=true`, startup is rejected with an error. This design means a misconfigured `KAFKA_SECURITY_PROTOCOL` env var cannot silently downgrade security.

---

## Contract Security

### Secrets by reference

Contract files (`contract.toml`) never contain secrets directly. Instead, they reference the **name** of the env var that holds the secret:

```toml
[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
token_env = "OPENCLAW_WEBHOOK_TOKEN"   # <-- env var name, not the token value
```

The actual token is loaded from the environment at runtime. This means contracts can be committed to version control without exposing secrets.

### Fail-closed validation

The contract validator has two categories of checks:
- **Security-critical** (always enforced): missing auth keys, unknown active drivers, invalid required fields, missing transport references, no smash outputs without explicit policy.
- **Non-security** (relaxed in debug mode): empty profile labels.

`--validation-mode debug` cannot relax security-critical checks. `--force` cannot bypass them either.

Unknown keys anywhere in a contract (adapter config, transport config) are always security-critical errors, regardless of mode.

---

## Rate Limiting

Serve applies two independent rate limiters:

| Limiter | Variable | Default | Scope |
|---|---|---|---|
| IP rate limit | `RELAY_IP_RATE_PER_MINUTE` | 100 | Per client IP address |
| Source rate limit | `RELAY_SOURCE_RATE_PER_MINUTE` | 500 | Per webhook source |

Both limiters are applied before signature validation.

### Proxy-aware IP extraction

By default, serve uses the direct TCP peer address for rate limiting. When behind a trusted reverse proxy:

```bash
RELAY_TRUST_PROXY_HEADERS=true
RELAY_TRUSTED_PROXY_CIDRS=10.0.0.0/8,172.16.0.0/12
```

Only requests arriving from the listed CIDRs will have their `X-Forwarded-For` / `X-Real-IP` headers trusted. Requests from untrusted IPs are rate-limited by their actual peer address. `RELAY_TRUSTED_PROXY_CIDRS` cannot be empty when proxy trust is enabled.

---

## Deduplication and Replay Protection

### Event deduplication

Serve tracks event IDs in an in-memory TTL store (`RELAY_DEDUP_TTL_SECONDS`, default 7 days). If the same event ID is seen again within the window, the duplicate is dropped silently with a 200 response (to prevent the sender from retrying indefinitely).

Dedup keys are source-specific:
- GitHub: derived from `X-GitHub-Delivery` header + `action` + entity ID
- Linear: derived from source-specific delivery ID

### Cooldown

Serve applies a per-entity cooldown (`RELAY_COOLDOWN_SECONDS`, default 30s) to suppress bursts of repeated events for the same entity (e.g. rapid PR updates). The cooldown key is source-specific:
- GitHub: `cooldown-github-<repo>-<entity_id>`
- Linear: source-specific entity identifier

### Linear timestamp window

Linear's timestamp window (`RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS`, default 60s) rejects webhooks delivered more than 60 seconds after their claimed timestamp. This provides replay protection for Linear events at the validation boundary.

---

## Sanitization

All payloads pass through `relay_core::sanitize` before envelope creation. The sanitizer applies zero-trust normalization to the JSON payload. It is not skipped even in debug validation mode.

See `docs/references/payload-sanitization.md` for the full sanitization rules.

---

## Destination Token Security

Smash egress adapters that require a bearer token (e.g. `openclaw_http_output`) reference the token via `token_env`. The actual token value is read from the environment at runtime and is never:
- Stored in the contract file
- Written to logs
- Exposed in health or debug endpoints

---

## Security Checklist for Production

- [ ] `KAFKA_SECURITY_PROTOCOL=ssl` (never plaintext in production)
- [ ] TLS certificate paths set and files readable by the process
- [ ] mTLS client certs generated via `scripts/gen-certs.sh`
- [ ] `HMAC_SECRET_GITHUB` and `HMAC_SECRET_LINEAR` set to strong random values (32+ bytes)
- [ ] Source secrets rotated in both hook env and the webhook provider's settings simultaneously
- [ ] `RELAY_TRUST_PROXY_HEADERS=false` unless behind a known reverse proxy, with `RELAY_TRUSTED_PROXY_CIDRS` set precisely
- [ ] `RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW=true` (default — do not disable)
- [ ] `OPENCLAW_WEBHOOK_TOKEN` (or any token_env value) set to a strong random token
- [ ] `RELAY_VALIDATION_MODE=strict` (default — do not change to debug in production)
- [ ] Kafka controller port (9093) not exposed beyond the private network
- [ ] `auto.create.topics.enable=false` in Kafka config (set in bootstrap script)
