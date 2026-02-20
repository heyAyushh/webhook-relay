use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use relay_core::keys::{
    github_cooldown_key, github_dedup_key, linear_cooldown_key, linear_dedup_key,
};
use relay_core::model::Source;
use relay_core::sanitize::sanitize_payload;
use relay_core::timestamps::verify_linear_timestamp_window;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use webhook_relay::client_ip::TrustedClientIpKeyExtractor;
use webhook_relay::config::Config;
use webhook_relay::envelope::build_envelope;
use webhook_relay::idempotency::{IdempotencyDecision, IdempotencyStore};
use webhook_relay::middleware::SourceRateLimiter;
use webhook_relay::producer::{
    KafkaPublisher, PublishJob, ensure_required_topics, run_publish_worker,
};
use webhook_relay::sources::{ValidationError, github, linear};

#[derive(Clone)]
struct AppState {
    config: Config,
    publish_tx: mpsc::Sender<PublishJob>,
    source_rate_limiter: SourceRateLimiter,
    idempotency_store: IdempotencyStore,
    publish_worker_alive: Arc<AtomicBool>,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing();

    let config = Config::from_env().context("load relay config")?;
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
    let source = match Source::from_str(&source_path) {
        Ok(source) => source,
        Err(_) => return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"}))),
    };

    if !state
        .source_rate_limiter
        .allow(source.as_str(), epoch_seconds())
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"source rate limit exceeded"})),
        );
    }

    if let Err(error) = validate_source(&state.config, source, &headers, &body) {
        match error {
            ValidationError::Unauthorized(message) => {
                warn!(
                    source = source.as_str(),
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
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid json payload"})),
            );
        }
    };

    if source == Source::Linear
        && !verify_linear_timestamp_window(
            &payload,
            epoch_seconds(),
            state.config.linear_timestamp_window_seconds,
            state.config.enforce_linear_timestamp_window,
        )
    {
        warn!(
            remote = %remote_addr.ip(),
            "linear webhook rejected due to timestamp window check"
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }

    let event_type = match event_type_for_source(source, &headers, &payload) {
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

    let dedup_key = match dedup_key_for_source(source, &headers, &payload) {
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
    let cooldown_key = cooldown_key_for_source(source, &payload);
    match state
        .idempotency_store
        .check(&dedup_key, cooldown_key.as_deref(), epoch_seconds())
    {
        IdempotencyDecision::Accept => {}
        IdempotencyDecision::Duplicate => {
            return (
                StatusCode::OK,
                Json(json!({"status":"ignored","reason":"duplicate"})),
            );
        }
        IdempotencyDecision::Cooldown => {
            return (
                StatusCode::OK,
                Json(json!({"status":"ignored","reason":"cooldown"})),
            );
        }
    }

    let sanitized_payload = match sanitize_payload(source.as_str(), &payload) {
        Ok(sanitized_payload) => sanitized_payload,
        Err(error) => {
            warn!(
                source = source.as_str(),
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

    let envelope = build_envelope(source, event_type, sanitized_payload);
    let topic = source.topic_name().to_string();

    let event_id = envelope.id.clone();
    let publish_job = PublishJob { topic, envelope };
    match state.publish_tx.try_send(publish_job) {
        Ok(()) => (StatusCode::OK, Json(json!({"status":"ok","id": event_id}))),
        Err(mpsc::error::TrySendError::Full(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"publisher queue is full"})),
        ),
        Err(mpsc::error::TrySendError::Closed(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"publisher unavailable"})),
        ),
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

fn validate_source(
    config: &Config,
    source: Source,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ValidationError> {
    match source {
        Source::Github => github::validate(&config.hmac_secret_github, headers, body),
        Source::Linear => linear::validate(&config.hmac_secret_linear, headers, body),
    }
}

fn event_type_for_source(
    source: Source,
    headers: &HeaderMap,
    payload: &Value,
) -> Result<String, ValidationError> {
    match source {
        Source::Github => github::event_type(headers, payload),
        Source::Linear => linear::event_type(headers, payload),
    }
}

fn dedup_key_for_source(
    source: Source,
    headers: &HeaderMap,
    payload: &Value,
) -> Result<String, ValidationError> {
    match source {
        Source::Github => {
            let delivery_id = header_value(headers, "X-GitHub-Delivery")
                .ok_or(ValidationError::BadRequest("missing X-GitHub-Delivery"))?;
            let action =
                payload_token(payload, &["action"]).unwrap_or_else(|| "unknown".to_string());
            let entity_id = github_entity_id(payload);
            Ok(github_dedup_key(&delivery_id, &action, &entity_id))
        }
        Source::Linear => {
            let delivery_id = header_value(headers, "Linear-Delivery")
                .ok_or(ValidationError::BadRequest("missing Linear-Delivery"))?;
            let action =
                payload_token(payload, &["action"]).unwrap_or_else(|| "unknown".to_string());
            let entity_id = linear_entity_id(payload);
            Ok(linear_dedup_key(&delivery_id, &action, &entity_id))
        }
    }
}

fn cooldown_key_for_source(source: Source, payload: &Value) -> Option<String> {
    match source {
        Source::Github => {
            let repo = payload_token(payload, &["repository", "full_name"])?;
            let entity_id = github_entity_id_for_cooldown(payload)?;
            Some(github_cooldown_key(&repo, &entity_id))
        }
        Source::Linear => {
            let team_key = payload_token(payload, &["data", "team", "key"])?;
            let entity_id = linear_entity_id_for_cooldown(payload)?;
            Some(linear_cooldown_key(&team_key, &entity_id))
        }
    }
}

fn github_entity_id(payload: &Value) -> String {
    github_entity_id_for_cooldown(payload)
        .or_else(|| payload_token(payload, &["comment", "id"]))
        .or_else(|| payload_token(payload, &["review", "id"]))
        .or_else(|| payload_token(payload, &["repository", "id"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn github_entity_id_for_cooldown(payload: &Value) -> Option<String> {
    payload_token(payload, &["pull_request", "number"])
        .or_else(|| payload_token(payload, &["issue", "number"]))
        .or_else(|| payload_token(payload, &["number"]))
}

fn linear_entity_id(payload: &Value) -> String {
    linear_entity_id_for_cooldown(payload)
        .or_else(|| payload_token(payload, &["webhookId"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn linear_entity_id_for_cooldown(payload: &Value) -> Option<String> {
    payload_token(payload, &["data", "id"])
        .or_else(|| payload_token(payload, &["data", "identifier"]))
}

fn payload_token(payload: &Value, path: &[&str]) -> Option<String> {
    let mut current = payload;
    for segment in path {
        current = current.get(*segment)?;
    }

    if let Some(value) = current.as_str() {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else if let Some(value) = current.as_i64() {
        Some(value.to_string())
    } else {
        current.as_u64().map(|value| value.to_string())
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
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

fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::{github_entity_id, ip_refill_period_ms, linear_entity_id, payload_token};
    use serde_json::json;

    #[test]
    fn ip_limit_refill_period_matches_100_per_minute() {
        assert_eq!(ip_refill_period_ms(100), 600);
    }

    #[test]
    fn github_entity_id_prefers_pull_request_number() {
        let payload = json!({
            "pull_request": {"number": 42},
            "number": 99
        });
        assert_eq!(github_entity_id(&payload), "42");
    }

    #[test]
    fn linear_entity_id_prefers_data_id() {
        let payload = json!({
            "data": {"id": "issue-42"},
            "webhookId": "hook-1"
        });
        assert_eq!(linear_entity_id(&payload), "issue-42");
    }

    #[test]
    fn payload_token_reads_integer_paths() {
        let payload = json!({"x":{"y":123}});
        assert_eq!(payload_token(&payload, &["x", "y"]).as_deref(), Some("123"));
    }
}
