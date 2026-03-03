use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use relay_core::sanitize::sanitize_payload;
use serde::Serialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tracing::{Level, debug, info, warn};
use tracing_subscriber::EnvFilter;
use webhook_relay::client_ip::TrustedClientIpKeyExtractor;
use webhook_relay::config::Config;
use webhook_relay::envelope::build_envelope;
use webhook_relay::idempotency::{IdempotencyDecision, IdempotencyStore};
use webhook_relay::middleware::SourceRateLimiter;
use webhook_relay::producer::{
    KafkaPublisher, PublishJob, ensure_required_topics, run_publish_worker,
};
use webhook_relay::sources::{
    ValidationError, handler_for_source, has_handler, known_source_names, normalize_source_name,
};

#[derive(Clone)]
struct AppState {
    config: Config,
    publish_tx: mpsc::Sender<PublishJob>,
    source_rate_limiter: SourceRateLimiter,
    idempotency_store: IdempotencyStore,
    publish_worker_alive: Arc<AtomicBool>,
}

const MAX_RAW_BODY_PREVIEW_CHARS: usize = 4_096;

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing();

    let config = Config::from_env().context("load relay config")?;
    ensure_enabled_sources_have_handlers(&config).context("validate enabled sources")?;
    if config.kafka_security_protocol == "plaintext" {
        warn!(
            "kafka plaintext transport is enabled (KAFKA_ALLOW_PLAINTEXT=true); use only on trusted private links"
        );
    }
    ensure_required_topics(&config)
        .await
        .context("ensure kafka topics")?;
    let publisher = KafkaPublisher::from_config(&config).context("initialize kafka producer")?;

    let (publish_tx, publish_rx) = mpsc::channel(config.publish_queue_capacity);
    let publish_worker_alive = Arc::new(AtomicBool::new(true));
    let publish_worker_alive_for_task = publish_worker_alive.clone();
    let publish_worker_handle = tokio::spawn(async move {
        run_publish_worker(publish_rx, publisher).await;
        publish_worker_alive_for_task.store(false, Ordering::SeqCst);
    });

    let state = Arc::new(AppState {
        source_rate_limiter: SourceRateLimiter::new(config.source_limit_per_minute),
        idempotency_store: IdempotencyStore::new(config.dedup_ttl_seconds, config.cooldown_seconds),
        config,
        publish_tx,
        publish_worker_alive,
    });

    let period_ms = ip_refill_period_ms(state.config.ip_limit_per_minute);
    let ip_key_extractor = TrustedClientIpKeyExtractor::new(
        state.config.trust_proxy_headers,
        state.config.trusted_proxy_cidrs.clone(),
    );
    let mut governor_builder = GovernorConfigBuilder::default()
        .key_extractor(ip_key_extractor)
        .use_headers();
    governor_builder
        .per_millisecond(period_ms)
        .burst_size(state.config.ip_limit_per_minute)
        .methods(vec![Method::POST]);
    let governor_config = Arc::new(
        governor_builder
            .finish()
            .ok_or_else(|| anyhow::anyhow!("build governor config"))?,
    );

    let app = Router::new()
        .route("/webhook/{source}", post(webhook_handler))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .layer(DefaultBodyLimit::max(state.config.max_payload_bytes))
        .layer(GovernorLayer::new(governor_config))
        .with_state(state.clone());

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .with_context(|| format!("bind {}", state.config.bind_addr))?;

    info!(
        bind = %state.config.bind_addr,
        trust_proxy_headers = state.config.trust_proxy_headers,
        trusted_proxy_cidrs = ?state.config.trusted_proxy_cidrs,
        "webhook relay listening"
    );

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    });

    server.await.context("serve webhook relay")?;

    drop(state);
    match timeout(Duration::from_secs(30), publish_worker_handle).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            warn!(error = %error, "publish worker exited with join error");
        }
        Err(_) => {
            warn!("timed out waiting for publish worker drain during shutdown");
        }
    }

    Ok(())
}

async fn webhook_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Path(source_path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let Some(normalized_source) = normalize_source_name(&source_path) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})));
    };
    if !state.config.is_source_enabled(&normalized_source) {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})));
    }
    let Some(handler) = handler_for_source(&normalized_source) else {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})));
    };
    let source = handler.source_name();
    let now_epoch_seconds = epoch_seconds();
    info!(
        source,
        remote = %remote_addr.ip(),
        body_bytes = body.len(),
        "webhook request received"
    );

    if !state.source_rate_limiter.allow(source, now_epoch_seconds) {
        warn!(
            source,
            remote = %remote_addr.ip(),
            "source rate limit exceeded"
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"source rate limit exceeded"})),
        );
    }

    if let Err(error) = handler.validate_request(&state.config, &headers, &body) {
        match error {
            ValidationError::Unauthorized(message) => {
                warn!(
                    source,
                    remote = %remote_addr.ip(),
                    reason = message,
                    "webhook authentication failed"
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":"unauthorized"})),
                );
            }
            ValidationError::BadRequest(message) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": message})));
            }
        }
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_error) => {
            if tracing::enabled!(Level::DEBUG) {
                debug!(
                    source,
                    remote = %remote_addr.ip(),
                    raw_body = %body_utf8_preview(&body, MAX_RAW_BODY_PREVIEW_CHARS),
                    "failed to parse webhook json payload"
                );
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid json payload"})),
            );
        }
    };
    debug!(
        source,
        remote = %remote_addr.ip(),
        webhook_payload = %payload,
        "parsed webhook payload"
    );

    if let Err(error) = handler.validate_payload(&state.config, &payload, now_epoch_seconds) {
        match error {
            ValidationError::Unauthorized(message) => {
                warn!(
                    source,
                    remote = %remote_addr.ip(),
                    reason = message,
                    "webhook payload validation failed"
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error":"unauthorized"})),
                );
            }
            ValidationError::BadRequest(message) => {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": message})));
            }
        }
    }

    let event_type = match handler.event_type(&headers, &payload) {
        Ok(event_type) => event_type,
        Err(ValidationError::BadRequest(message)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message})));
        }
        Err(ValidationError::Unauthorized(_)) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized"})),
            );
        }
    };
    debug!(
        source,
        event_type = event_type.as_str(),
        "derived webhook event type"
    );

    let dedup_key = match handler.dedup_key(&headers, &payload) {
        Ok(key) => key,
        Err(ValidationError::BadRequest(message)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": message})));
        }
        Err(ValidationError::Unauthorized(_)) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized"})),
            );
        }
    };
    let cooldown_key = handler.cooldown_key(&payload);
    debug!(
        source,
        dedup_key = dedup_key.as_str(),
        cooldown_key = ?cooldown_key,
        "computed idempotency keys"
    );
    match state
        .idempotency_store
        .check(&dedup_key, cooldown_key.as_deref(), now_epoch_seconds)
    {
        IdempotencyDecision::Accept => {}
        IdempotencyDecision::Duplicate => {
            info!(
                source,
                dedup_key = dedup_key.as_str(),
                "ignored duplicate webhook delivery"
            );
            return (
                StatusCode::OK,
                Json(json!({"status":"ignored","reason":"duplicate"})),
            );
        }
        IdempotencyDecision::Cooldown => {
            info!(
                source,
                cooldown_key = ?cooldown_key,
                "ignored webhook due to cooldown"
            );
            return (
                StatusCode::OK,
                Json(json!({"status":"ignored","reason":"cooldown"})),
            );
        }
    }

    let sanitized_payload = match sanitize_payload(source, &payload) {
        Ok(sanitized_payload) => sanitized_payload,
        Err(error) => {
            warn!(
                source,
                remote = %remote_addr.ip(),
                reason = %error,
                "payload sanitizer rejected request"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid payload"})),
            );
        }
    };
    debug!(
        source,
        sanitized_payload = %sanitized_payload,
        "sanitized webhook payload"
    );

    let envelope = build_envelope(source, event_type, sanitized_payload);
    let topic = handler.topic_name(&state.config);
    debug!(
        source,
        topic = topic.as_str(),
        envelope_json = %to_json_string(&envelope),
        "prepared kafka publish envelope"
    );

    let event_id = envelope.id.clone();
    let event_type_for_log = envelope.event_type.clone();
    let topic_for_log = topic.clone();
    let publish_job = PublishJob { topic, envelope };
    match state.publish_tx.try_send(publish_job) {
        Ok(()) => {
            info!(
                source,
                event_type = event_type_for_log.as_str(),
                topic = topic_for_log.as_str(),
                event_id = event_id.as_str(),
                remote = %remote_addr.ip(),
                "webhook event accepted and queued for kafka publish"
            );
            (StatusCode::OK, Json(json!({"status":"ok","id": event_id})))
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            warn!(
                source,
                topic = topic_for_log.as_str(),
                event_id = event_id.as_str(),
                "failed to enqueue webhook envelope: publisher queue is full"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"publisher queue is full"})),
            )
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            warn!(
                source,
                topic = topic_for_log.as_str(),
                event_id = event_id.as_str(),
                "failed to enqueue webhook envelope: publisher unavailable"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":"publisher unavailable"})),
            )
        }
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.publish_worker_alive.load(Ordering::SeqCst) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"not_ready","reason":"publisher worker not running"})),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "ready",
            "bind": state.config.bind_addr,
            "version": env!("CARGO_PKG_VERSION")
        })),
    )
}

fn ensure_enabled_sources_have_handlers(config: &Config) -> Result<()> {
    let unsupported = config
        .enabled_sources
        .iter()
        .filter(|source| !has_handler(source))
        .cloned()
        .collect::<Vec<_>>();

    if unsupported.is_empty() {
        return Ok(());
    }

    let built_ins = known_source_names().join(", ");
    Err(anyhow::anyhow!(
        "enabled sources without handlers: {} (built-in handlers: {})",
        unsupported.join(", "),
        built_ins
    ))
}

fn ip_refill_period_ms(limit_per_minute: u32) -> u64 {
    if limit_per_minute == 0 {
        return 1;
    }

    let period = 60_000u64 / u64::from(limit_per_minute);
    period.max(1)
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn body_utf8_preview(body: &Bytes, max_chars: usize) -> String {
    let raw = String::from_utf8_lossy(body);
    if raw.chars().count() <= max_chars {
        return raw.into_owned();
    }

    let preview_limit = max_chars.saturating_sub(3);
    let mut output = String::new();
    let mut char_count = 0usize;
    for character in raw.chars() {
        if char_count >= preview_limit {
            break;
        }
        output.push(character);
        char_count = char_count.saturating_add(1);
    }
    output.push_str("...");
    output
}

fn to_json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{}\"}}", error))
}

fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::ip_refill_period_ms;

    #[test]
    fn ip_limit_refill_period_matches_100_per_minute() {
        assert_eq!(ip_refill_period_ms(100), 600);
    }

    #[test]
    fn ip_limit_refill_period_has_minimum_one_millisecond() {
        assert_eq!(ip_refill_period_ms(100_000), 1);
    }

    #[test]
    fn ip_limit_refill_period_handles_zero_limit() {
        assert_eq!(ip_refill_period_ms(0), 1);
    }
}
