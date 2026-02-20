use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use relay_core::model::Source;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use webhook_relay::config::Config;
use webhook_relay::envelope::build_envelope;
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
        config,
        publish_tx,
        publish_worker_alive,
    });

    let period_ms = ip_refill_period_ms(state.config.ip_limit_per_minute);
    let mut governor_builder = GovernorConfigBuilder::default()
        .key_extractor(SmartIpKeyExtractor)
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

    info!(bind = %state.config.bind_addr, "webhook relay listening");

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    });

    server.await.context("serve webhook relay")?;

    drop(state);
    publish_worker_handle.abort();
    let _ = publish_worker_handle.await;

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

    let envelope = build_envelope(source, event_type, payload);
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
    use super::ip_refill_period_ms;

    #[test]
    fn ip_limit_refill_period_matches_100_per_minute() {
        assert_eq!(ip_refill_period_ms(100), 600);
    }
}
