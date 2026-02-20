# webhook-relay Rust Rewrite Proposal v2.2

**Date:** February 20, 2026  
**Author:** Grok (on behalf of the OpenClaw team)  
**Version:** 2.2  
**Status:** Approved & Ready for Implementation  
**Repo:** https://github.com/heyAyushh/webhook-relay

## Version History

| Version | Date | Change |
| --- | --- | --- |
| 2.2 | 2026-02-20 | Full feature parity matrix, explicit repo behavior contract, strict parity acceptance checklist |
| 2.1 | 2026-02-20 | Retry correctness fix, rate limiting, observability, graceful shutdown, admin endpoints, Firecracker hardening |
| 2.0 | 2026-02-20 | Queue-first architecture and durable queue/DLQ plan |

## Executive Summary

This version defines exact behavioral parity with the current relay while keeping the rewrite light, secure, and resilient.

- Security-first isolation remains mandatory (separate environment from OpenClaw agents).
- Queue-first architecture remains mandatory (fast ACK + durable delivery).
- Feature parity is defined as a testable contract, not a claim.
- Excellence features (rate limit, observability, graceful shutdown, DLQ replay, hardening) remain included.

**Core Security Decision (non-negotiable):**  
Relay runs on a separate isolated environment (Firecracker microVM preferred). A relay compromise must not provide lateral access to OpenClaw agents.

**Chosen Deployment (lightest + secure):**  
Firecracker microVM preferred, fallback tiny VPS with LUKS-encrypted persistent data volume.  
Target footprint: ~10-18 MB static binary + single `redb` database file.

## Feature Parity Matrix (Behavioral Contract)

| Feature / Behavior | Current Repo Behavior | Rust v2.2 Requirement | Parity Gate |
| --- | --- | --- | --- |
| GitHub ingress path and methods | `POST /hooks/github-pr` | Same route and method restrictions | Integration test |
| GitHub event/action filtering | Event in `{pull_request,pull_request_review,pull_request_review_comment,issue_comment}` and action regex `^(opened|synchronize|reopened|submitted|created)$` | Exact equivalent pre-enqueue filter | Integration test against fixtures |
| Linear ingress path and methods | `POST /hooks/linear` | Same route and method restrictions | Integration test |
| Linear type filtering | `type` in `{Issue, Comment}` | Exact equivalent pre-enqueue filter | Integration test |
| GitHub signature verification | HMAC-SHA256 using `X-Hub-Signature-256` | Raw-bytes constant-time verify before parse | Security test |
| Linear signature verification | HMAC-SHA256 using `Linear-Signature` | Raw-bytes constant-time verify before parse | Security test |
| Linear timestamp window | `LINEAR_TIMESTAMP_WINDOW_SECONDS` default `60`, `LINEAR_ENFORCE_TIMESTAMP_CHECK` default `true` | Exact same defaults and bypass toggle semantics | Unit + integration tests |
| GitHub bot skip | Sender login ends with `[bot]` -> drop | Exact same rule | Unit test |
| Linear self-actor skip | If `LINEAR_AGENT_USER_ID == LINEAR_ACTOR_ID` -> drop | Exact same rule | Unit test |
| Dedup key semantics | `github:{delivery}:{action}:{entity}` and `linear:{delivery}:{action}:{entity}` with retention cleanup | Exact same dedup key semantics in durable store | Concurrency + replay tests |
| Cooldown key semantics | GitHub key by `repo + entity`; Linear key by `team + entity`; default `30s` | Exact same cooldown keys and defaults | Time-window integration tests |
| Payload sanitization | Source-specific sanitize: allowlist extraction, fencing, regex flags, truncation, metadata flags | Output-equivalent sanitizer behavior for GitHub/Linear fixtures | Golden-file tests vs current Python |
| Forward URL and source query | `/hooks/agent?source=github-pr` and `/hooks/agent?source=linear` | Exact same target query semantics | Integration test |
| Forward headers (existing) | Preserve `Authorization`, `Content-Type`, `X-Webhook-Source`, GitHub/Linear delivery/event headers | Preserve existing headers; add enrichment headers non-breaking | Header parity tests |
| Retry and timeout controls | `WEBHOOK_CURL_CONNECT_TIMEOUT_SECONDS`, `WEBHOOK_CURL_MAX_TIME_SECONDS`, retry envs | Equivalent config/env controls with same defaults | Failure injection tests |
| Metrics counters | `webhook_relay_events_received_total`, `webhook_relay_events_forwarded_total`, `webhook_relay_events_dropped_total` with labels | Backward-compatible counters preserved | Metrics parity tests |
| Smoke-test outcome | One forwarded event per source after duplicate submissions; duplicate drop counters increment | Same observable outcomes | Parity smoke suite |
| Env surface parity | Existing relay env vars available | All existing envs supported (env override + config fallback) | Config/env compatibility test |

## Architecture (Queue-First)

GitHub / Linear  
-> raw bytes ingress (Axum)  
-> verify, filter, dedup, enqueue in one durable transaction  
-> `202 Accepted`  
-> async workers dequeue, sanitize, forward, retry, DLQ on exhaustion.

**Durable Store:** `redb` (single-file ACID).

Tables:
- `pending_events` (`event_id`, source, route, raw_payload, attempts, next_retry_at, created_at)
- `dedup_index` (composite key with TTL metadata)
- `cooldown_index` (entity cooldown key with expiry)
- `dlq_events` (`event_id`, failure_reason, failed_at, replay_count)
- `audit_log` (append-only security and admin actions)

## Corrected Retry Logic (Production Grade)

```rust
// worker.rs (illustrative)
let operation = || async {
    let response = match client
        .post(&target_url)
        .json(&sanitized_payload)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            if error.is_connect() || error.is_timeout() || error.is_request() {
                return Err(backoff::Error::transient(error));
            }
            return Err(backoff::Error::permanent(error));
        }
    };

    let status = response.status();
    if status.is_server_error() || status.as_u16() == 429 {
        return Err(backoff::Error::transient(anyhow::anyhow!(
            "retryable upstream status {}",
            status
        )));
    }

    if status.is_client_error() {
        return Err(backoff::Error::permanent(anyhow::anyhow!(
            "permanent upstream status {}",
            status
        )));
    }

    Ok(())
};

backoff::future::retry(
    ExponentialBackoff {
        initial_interval: Duration::from_millis(500),
        max_interval: Duration::from_secs(60),
        max_elapsed_time: Some(Duration::from_secs(300)),
        ..Default::default()
    },
    operation,
)
.await?;
```

Configurable per route (`max_attempts`, `max_elapsed_time`) with jitter. On exhaustion, event moves to DLQ.

## Rate Limiting + Cooldown

- Global ingress rate limiting required by default.
- Per-IP and per-route buckets (for example, `tower-governor`).
- Optional per-secret limiter for leaked endpoint abuse.
- Existing per-entity cooldown behavior is preserved exactly (GitHub `repo+entity`, Linear `team+entity`, default `30s`).

## Dynamic Routing, Hot-Reload, Filtering

- Single catch-all route: `/hooks/*path`.
- `config.toml` `[[routes]]` maps path -> source type -> target -> filters.
- Reload via SIGHUP or file watcher.
- Reload semantics: parse + validate + atomic swap; keep previous config on failure.

## Protected Admin Endpoints

- Exposed only on private/tailnet interface.
- Require admin bearer token (`WEBHOOK_ADMIN_TOKEN`) and constant-time check.
- Endpoints:
- `GET /admin/queue`
- `GET /admin/dlq`
- `POST /admin/dlq/replay/{event_id}`
- All admin actions recorded in `audit_log`.

## OpenClaw Forward Enrichment

Existing forward headers are preserved. Additional headers:
- `X-OpenClaw-Event-ID`
- `X-OpenClaw-Sanitized`
- `X-OpenClaw-Risk-Score`

## Health, Readiness, Observability

- `GET /health`: liveness only.
- `GET /ready`: checks local dependencies only (config, db writable, workers healthy, disk threshold).
- Structured tracing with correlation IDs.
- Prometheus metrics include both:
- backward-compatible counters from current repo
- new queue/retry/DLQ/rate-limit/latency metrics

## Graceful Shutdown

On `SIGTERM`/`SIGINT`:
- stop accepting new ingress
- finish in-flight verification/enqueue
- drain workers with bounded timeout
- keep uncompleted leased events retryable after restart via `next_retry_at`

## Realistic SLOs

- `99.9%` uptime for single-node tier
- p99 ACK latency < 400 ms
- No accepted-event loss (at-least-once delivery, dedup, DLQ, replay)

## Dependency Policy (Lightweight Only)

Core crates: `axum`, `tokio`, `redb`, `hmac`, `sha2`, `subtle`, `reqwest` (minimal rustls), `backoff`, `serde`, `tracing`, `prometheus`.

No heavyweight dependencies unless justified by measurable security or reliability gain.

## Exhaustive Edge-Case Coverage

| Category | Edge Case | Mitigation | Verification |
| --- | --- | --- | --- |
| Auth | Tampered body / missing signature | Raw-bytes HMAC reject before parse | Security tests |
| GitHub-specific | Ping or unsupported action | Drop by exact event/action filters | Fixture tests |
| GitHub-specific | Sender ends with `[bot]` | Bot-skip rule | Unit test |
| Linear-specific | Timestamp skew exceeds window | Reject when timestamp enforcement enabled | Time-window tests |
| Linear-specific | Missing `Linear-Delivery` or invalid signature | Reject and audit | Integration tests |
| Dedup | Concurrent duplicate deliveries | Atomic dedup insert with canonical key | Concurrency tests |
| Cooldown | Burst of different deliveries same entity | Cooldown drop by repo/team + entity | Time-window tests |
| Sanitizer | Source-specific allowlist mismatch | Golden tests against Python sanitizer outputs | Golden parity tests |
| Forward | Upstream 429/5xx/transient network | Retry with jitter then DLQ | Fault injection tests |
| Config | Invalid hot reload | Keep old config and emit error | Reload failure tests |
| Storage | Disk pressure | `/ready` fails + enqueue backpressure | Disk pressure tests |
| Abuse | Rate-limit burst attacks | Per-IP/per-route limiter returns 429 | Stress tests |
| Replay | Concurrent DLQ replay requests | Idempotent replay + audit log | Replay race tests |

## Firecracker Hardening Baseline

- Firecracker Jailer required.
- Seccomp filters + cgroup CPU/memory limits.
- Read-only root filesystem; encrypted writable data volume only.
- Drop unnecessary capabilities.
- Disable vsock by default.
- Egress restricted to required destinations.

## Containerization Plan

Containerization is part of the implementation scope.

- Production image:
- Multi-stage `Dockerfile` (Rust builder -> minimal runtime image).
- Non-root user, read-only root filesystem, writable mount only for `redb` data.
- Healthcheck endpoint wired to `/health`.
- Image built with minimal features and stripped binary target.
- Development profile:
- `docker-compose.yml` for local integration testing.
- Includes relay service + optional mock OpenClaw target.
- Mounts config and temporary data volumes for parity/smoke tests.
- Security posture:
- No secrets baked into images.
- Runtime secrets injected via environment variables or secret mounts.
- Minimal Linux capabilities and explicit resource limits in compose/Kubernetes manifests.

## Parity Acceptance Checklist (Must Pass Before Cutover)

1. Hook route and filter parity tests pass for GitHub and Linear fixtures.
2. Dedup and cooldown semantics match existing scripts exactly.
3. Sanitizer golden outputs match current Python sanitizer for canonical test corpus.
4. Forward URL and required headers parity pass.
5. Existing metrics counters and labels match smoke expectations.
6. Linear timestamp enforcement and bypass toggle behavior matches existing env semantics.
7. Smoke parity suite reproduces current outcomes (forward once, duplicate dropped, counters incremented).
8. Chaos tests for restart, transient failure, and replay idempotency pass.

## Migration Plan (Zero Downtime)

1. Deploy Rust relay in parallel on isolated host/microVM.
2. Run full parity suite and signed-event manual validation.
3. Shift GitHub and Linear webhooks incrementally.
4. Monitor queue depth, retry rate, DLQ growth, and parity counters for 72 hours.
5. Decommission legacy relay after stability window.

## Cost

- Firecracker microVM / tiny VPS: $4-6 per month
- No mandatory additional SaaS cost

## Next Steps

1. Merge `proposal.md` as v2.2.
2. Provision Firecracker microVM (or encrypted VPS fallback).
3. Scaffold Rust service with parity tests first, including `Dockerfile` and `docker-compose.yml`.
4. Implement in 2-3 weeks FTE equivalent.
