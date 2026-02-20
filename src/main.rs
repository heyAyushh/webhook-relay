use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use webhook_relay::config::Config;
use webhook_relay::filters::{is_supported_github_event_action, is_supported_linear_type};
use webhook_relay::keys::{
    github_cooldown_key, github_dedup_key, linear_cooldown_key, linear_dedup_key,
};
use webhook_relay::metrics::Metrics;
use webhook_relay::model::{EnqueueResult, EventMetadata, PendingEvent, Source};
use webhook_relay::sanitize::sanitize_payload;
use webhook_relay::signatures::{verify_github_signature, verify_linear_signature};
use webhook_relay::store::RelayStore;
use webhook_relay::timestamps::verify_linear_timestamp_window;

#[derive(Clone)]
struct AppState {
    config: Config,
    store: RelayStore,
    metrics: Metrics,
    http_client: Client,
    worker_notify: Arc<Notify>,
}

#[derive(Debug)]
enum ForwardAttemptOutcome {
    Success,
    Transient(String),
    Permanent(String),
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing();

    let config = Config::from_env().context("load config from environment")?;
    let store = RelayStore::open(&config.db_path).context("open relay store")?;
    let metrics = Metrics::new().context("initialize metrics")?;

    let http_client = Client::builder()
        .connect_timeout(Duration::from_secs(config.http_connect_timeout_seconds))
        .timeout(Duration::from_secs(config.http_request_timeout_seconds))
        .build()
        .context("build HTTP client")?;

    let state = Arc::new(AppState {
        config,
        store,
        metrics,
        http_client,
        worker_notify: Arc::new(Notify::new()),
    });

    refresh_queue_metrics(&state);

    let app = Router::new()
        .route("/hooks/github-pr", post(github_hook))
        .route("/hooks/linear", post(linear_hook))
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/metrics", get(metrics_endpoint))
        .route("/admin/queue", get(admin_queue))
        .route("/admin/dlq", get(admin_dlq))
        .route("/admin/dlq/replay/{event_id}", post(admin_replay))
        .layer(DefaultBodyLimit::max(state.config.ingress_max_body_bytes))
        .with_state(state.clone());

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .with_context(|| format!("bind {}", state.config.bind_addr))?;

    info!(bind_addr = %state.config.bind_addr, "webhook relay listening");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let worker_state = state.clone();
    let worker_handle = tokio::spawn(async move {
        worker_loop(worker_state, shutdown_rx).await;
    });

    let shutdown_for_server = shutdown_tx.clone();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            info!("shutdown signal received");
        }
        let _ = shutdown_for_server.send(true);
    });

    server.await.context("serve axum application")?;
    let _ = shutdown_tx.send(true);
    state.worker_notify.notify_waiters();

    worker_handle.await.context("join worker task")?;

    Ok(())
}

async fn worker_loop(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<bool>) {
    let poll_interval = Duration::from_millis(state.config.queue_poll_interval_ms);

    loop {
        tokio::select! {
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            _ = state.worker_notify.notified() => {}
            _ = tokio::time::sleep(poll_interval) => {}
        }

        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            let now = epoch_seconds();
            let maybe_event = match state.store.pop_due_event(now) {
                Ok(event) => event,
                Err(error) => {
                    error!(error = %error, "failed to pop due event");
                    break;
                }
            };

            let Some(event) = maybe_event else {
                break;
            };

            process_pending_event(state.clone(), event).await;
            refresh_queue_metrics(&state);
        }
    }

    info!("worker loop stopped");
}

async fn process_pending_event(state: Arc<AppState>, mut event: PendingEvent) {
    let source_label = event.source.as_str();
    let sanitized_payload = match sanitize_payload(source_label, &event.payload) {
        Ok(payload) => payload,
        Err(error) => {
            error!(event_id = %event.event_id, error = %error, "sanitize failed");
            if let Err(dlq_error) =
                state
                    .store
                    .move_to_dlq(event, "sanitization_failed", epoch_seconds())
            {
                error!(error = %dlq_error, "failed to store sanitization failure in dlq");
            }
            state
                .metrics
                .inc_dropped(source_label, "sanitization_failed");
            return;
        }
    };

    match forward_once(&state, &event, &sanitized_payload).await {
        ForwardAttemptOutcome::Success => {
            state.metrics.inc_forwarded(source_label);
        }
        ForwardAttemptOutcome::Permanent(reason) => {
            warn!(event_id = %event.event_id, reason = %reason, "permanent forwarding failure");
            if let Err(error) = state
                .store
                .move_to_dlq(event, "forward_failed", epoch_seconds())
            {
                error!(error = %error, "failed to move permanent failure to dlq");
            }
            state.metrics.inc_dropped(source_label, "forward_failed");
        }
        ForwardAttemptOutcome::Transient(reason) => {
            event.attempts = event.attempts.saturating_add(1);
            if event.attempts >= state.config.forward_max_attempts {
                warn!(
                    event_id = %event.event_id,
                    attempts = event.attempts,
                    reason = %reason,
                    "transient forwarding exhausted retries"
                );
                if let Err(error) =
                    state
                        .store
                        .move_to_dlq(event, "forward_failed", epoch_seconds())
                {
                    error!(error = %error, "failed to move exhausted transient failure to dlq");
                }
                state.metrics.inc_dropped(source_label, "forward_failed");
                return;
            }

            let backoff_seconds = compute_backoff_seconds(
                state.config.forward_initial_backoff_seconds,
                state.config.forward_max_backoff_seconds,
                event.attempts,
            );
            event.next_retry_at_epoch = epoch_seconds() + backoff_seconds as i64;

            warn!(
                event_id = %event.event_id,
                attempts = event.attempts,
                backoff_seconds,
                reason = %reason,
                "transient forwarding failure, event requeued"
            );

            if let Err(error) = state.store.requeue_event(event) {
                error!(error = %error, "failed to requeue event after transient error");
            } else {
                state.worker_notify.notify_one();
            }
        }
    }
}

async fn forward_once(
    state: &AppState,
    event: &PendingEvent,
    sanitized_payload: &Value,
) -> ForwardAttemptOutcome {
    let mut target = state
        .config
        .openclaw_gateway_url
        .trim_end_matches('/')
        .to_string();
    target.push_str("/hooks/agent?source=");
    target.push_str(event.source.openclaw_source_query());

    let mut request = state
        .http_client
        .post(target)
        .header(
            "Authorization",
            format!("Bearer {}", state.config.openclaw_hooks_token),
        )
        .header("Content-Type", "application/json")
        .header("X-Webhook-Source", event.source.as_str())
        .header("X-OpenClaw-Event-ID", event.event_id.clone())
        .header("X-OpenClaw-Sanitized", "true")
        .header(
            "X-OpenClaw-Risk-Score",
            compute_risk_score(sanitized_payload).to_string(),
        )
        .json(sanitized_payload);

    match event.source {
        Source::Github => {
            if let Some(event_name) = &event.metadata.event_name {
                request = request.header("X-GitHub-Event", event_name);
            }
            request = request.header("X-GitHub-Delivery", &event.metadata.delivery_id);
            if let Some(installation_id) = &event.metadata.installation_id {
                request = request.header("X-GitHub-Installation", installation_id);
            }
        }
        Source::Linear => {
            if let Some(event_name) = &event.metadata.event_name {
                request = request.header("X-Linear-Event", event_name);
            }
            request = request.header("X-Linear-Delivery", &event.metadata.delivery_id);
        }
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            if error.is_connect() || error.is_timeout() || error.is_request() {
                return ForwardAttemptOutcome::Transient(error.to_string());
            }
            return ForwardAttemptOutcome::Permanent(error.to_string());
        }
    };

    let status = response.status();
    if status.is_success() {
        return ForwardAttemptOutcome::Success;
    }

    if status.is_server_error() || status.as_u16() == 429 {
        return ForwardAttemptOutcome::Transient(format!("upstream status {status}"));
    }

    ForwardAttemptOutcome::Permanent(format!("upstream status {status}"))
}

async fn github_hook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    state.metrics.inc_received("github");

    let signature = match header_string(&headers, "X-Hub-Signature-256") {
        Some(value) => value,
        None => {
            state.metrics.inc_dropped("github", "invalid_signature");
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"missing signature"})),
            );
        }
    };

    if !verify_github_signature(&state.config.github_webhook_secret, &body, &signature) {
        state.metrics.inc_dropped("github", "invalid_signature");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"invalid signature"})),
        );
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            state.metrics.inc_dropped("github", "invalid_payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid json: {error}")})),
            );
        }
    };

    let event_name = match header_string(&headers, "X-GitHub-Event") {
        Some(value) => value,
        None => {
            state.metrics.inc_dropped("github", "invalid_payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"missing X-GitHub-Event"})),
            );
        }
    };

    let delivery_id = match header_string(&headers, "X-GitHub-Delivery") {
        Some(value) => value,
        None => {
            state.metrics.inc_dropped("github", "invalid_payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"missing X-GitHub-Delivery"})),
            );
        }
    };

    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if action.is_empty() {
        state.metrics.inc_dropped("github", "invalid_payload");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"missing action in payload"})),
        );
    }

    if !is_supported_github_event_action(&event_name, &action) {
        state.metrics.inc_dropped("github", "filtered");
        return accepted("filtered");
    }

    let sender_login = payload
        .get("sender")
        .and_then(|sender| sender.get("login"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if sender_login.ends_with("[bot]") {
        state.metrics.inc_dropped("github", "bot_sender");
        return accepted("bot_sender");
    }

    let entity_id = resolve_github_entity_id(&payload);
    let repo_name = payload
        .get("repository")
        .and_then(|repository| repository.get("full_name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    let dedup_key = github_dedup_key(&delivery_id, &action, &entity_id);
    let cooldown_key = github_cooldown_key(repo_name, &entity_id);

    let installation_id = resolve_optional_string(&["installation", "id"], &payload);

    let event = PendingEvent {
        event_id: Uuid::new_v4().to_string(),
        source: Source::Github,
        dedup_key,
        cooldown_key,
        action,
        entity_id,
        payload,
        metadata: EventMetadata {
            delivery_id,
            event_name: Some(event_name),
            installation_id,
            team_key: None,
        },
        attempts: 0,
        next_retry_at_epoch: epoch_seconds(),
        created_at_epoch: epoch_seconds(),
    };

    match state.store.enqueue_pending_event(
        event,
        state.config.dedup_retention_seconds(),
        state.config.github_cooldown_seconds,
        epoch_seconds(),
    ) {
        Ok(EnqueueResult::Enqueued) => {
            state.worker_notify.notify_one();
            refresh_queue_metrics(&state);
            accepted("enqueued")
        }
        Ok(EnqueueResult::Duplicate) => {
            state.metrics.inc_dropped("github", "duplicate_delivery");
            accepted("duplicate_delivery")
        }
        Ok(EnqueueResult::Cooldown) => {
            state.metrics.inc_dropped("github", "cooldown");
            accepted("cooldown")
        }
        Err(error) => {
            error!(error = %error, "failed to enqueue github event");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"failed to enqueue event"})),
            )
        }
    }
}

async fn linear_hook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    state.metrics.inc_received("linear");

    let signature = match header_string(&headers, "Linear-Signature") {
        Some(value) => value,
        None => {
            state.metrics.inc_dropped("linear", "invalid_signature");
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"missing signature"})),
            );
        }
    };

    if !verify_linear_signature(&state.config.linear_webhook_secret, &body, &signature) {
        state.metrics.inc_dropped("linear", "invalid_signature");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"invalid signature"})),
        );
    }

    let payload: Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            state.metrics.inc_dropped("linear", "invalid_payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid json: {error}")})),
            );
        }
    };

    let delivery_id = match header_string(&headers, "Linear-Delivery") {
        Some(value) => value,
        None => {
            state.metrics.inc_dropped("linear", "invalid_payload");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"missing Linear-Delivery"})),
            );
        }
    };

    let event_type = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if event_type.is_empty() || action.is_empty() {
        state.metrics.inc_dropped("linear", "invalid_payload");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"missing type or action in payload"})),
        );
    }

    if !is_supported_linear_type(&event_type) {
        state.metrics.inc_dropped("linear", "filtered");
        return accepted("filtered");
    }

    if !verify_linear_timestamp_window(
        &payload,
        epoch_seconds(),
        state.config.linear_timestamp_window_seconds,
        state.config.linear_enforce_timestamp_check,
    ) {
        state.metrics.inc_dropped("linear", "invalid_timestamp");
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"invalid or stale webhookTimestamp"})),
        );
    }

    if let Some(agent_user_id) = &state.config.linear_agent_user_id {
        let actor_id = payload
            .get("data")
            .and_then(|data| data.get("userId"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !actor_id.is_empty() && actor_id == agent_user_id {
            state.metrics.inc_dropped("linear", "agent_user");
            return accepted("agent_user");
        }
    }

    let entity_id = payload
        .get("data")
        .and_then(|data| data.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let team_key = payload
        .get("data")
        .and_then(|data| data.get("team"))
        .and_then(|team| team.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let dedup_key = linear_dedup_key(&delivery_id, &action, &entity_id);
    let cooldown_key = linear_cooldown_key(&team_key, &entity_id);

    let event = PendingEvent {
        event_id: Uuid::new_v4().to_string(),
        source: Source::Linear,
        dedup_key,
        cooldown_key,
        action,
        entity_id,
        payload,
        metadata: EventMetadata {
            delivery_id,
            event_name: Some(event_type),
            installation_id: None,
            team_key: Some(team_key),
        },
        attempts: 0,
        next_retry_at_epoch: epoch_seconds(),
        created_at_epoch: epoch_seconds(),
    };

    match state.store.enqueue_pending_event(
        event,
        state.config.dedup_retention_seconds(),
        state.config.linear_cooldown_seconds,
        epoch_seconds(),
    ) {
        Ok(EnqueueResult::Enqueued) => {
            state.worker_notify.notify_one();
            refresh_queue_metrics(&state);
            accepted("enqueued")
        }
        Ok(EnqueueResult::Duplicate) => {
            state.metrics.inc_dropped("linear", "duplicate_delivery");
            accepted("duplicate_delivery")
        }
        Ok(EnqueueResult::Cooldown) => {
            state.metrics.inc_dropped("linear", "cooldown");
            accepted("cooldown")
        }
        Err(error) => {
            error!(error = %error, "failed to enqueue linear event");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"failed to enqueue event"})),
            )
        }
    }
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pending_count = match state.store.pending_count() {
        Ok(count) => count,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status":"not_ready","error":error.to_string()})),
            );
        }
    };

    let dlq_count = match state.store.dlq_count() {
        Ok(count) => count,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"status":"not_ready","error":error.to_string()})),
            );
        }
    };

    refresh_queue_metrics(&state);

    (
        StatusCode::OK,
        Json(json!({
            "status": "ready",
            "queue_depth": pending_count,
            "dlq_depth": dlq_count,
            "version": env!("CARGO_PKG_VERSION")
        })),
    )
}

async fn metrics_endpoint(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.metrics.render() {
        Ok(body) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            );
            (StatusCode::OK, headers, body)
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            HeaderMap::new(),
            format!("failed to render metrics: {error}"),
        ),
    }
}

async fn admin_queue(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(response) = require_admin_auth(&state, &headers) {
        return response;
    }

    let pending = state.store.pending_count().unwrap_or(0);
    let dlq = state.store.dlq_count().unwrap_or(0);
    refresh_queue_metrics(&state);

    (
        StatusCode::OK,
        Json(json!({"pending": pending, "dlq": dlq})),
    )
}

async fn admin_dlq(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(response) = require_admin_auth(&state, &headers) {
        return response;
    }

    match state.store.list_dlq_events(100) {
        Ok(events) => (StatusCode::OK, Json(json!({"events": events}))),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        ),
    }
}

async fn admin_replay(
    State(state): State<Arc<AppState>>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = require_admin_auth(&state, &headers) {
        return response;
    }

    match state.store.replay_dlq_event(&event_id, epoch_seconds()) {
        Ok(true) => {
            state.worker_notify.notify_one();
            refresh_queue_metrics(&state);
            (
                StatusCode::OK,
                Json(json!({"replayed": true, "event_id": event_id})),
            )
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({"replayed": false, "event_id": event_id})),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": error.to_string()})),
        ),
    }
}

fn require_admin_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<Value>)> {
    let Some(admin_token) = &state.config.admin_token else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"admin endpoints disabled"})),
        ));
    };

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let expected = format!("Bearer {admin_token}");

    if auth_header != expected {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        ));
    }

    Ok(())
}

fn refresh_queue_metrics(state: &AppState) {
    let pending = state.store.pending_count().unwrap_or(0);
    let dlq = state.store.dlq_count().unwrap_or(0);
    state.metrics.set_queue_depth(pending);
    state.metrics.set_dlq_depth(dlq);
}

fn setup_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn compute_backoff_seconds(initial_seconds: u64, max_seconds: u64, attempts: u32) -> u64 {
    let exponent = attempts.saturating_sub(1).min(31);
    let scaled = initial_seconds.saturating_mul(1u64 << exponent);
    scaled.min(max_seconds)
}

fn compute_risk_score(sanitized_payload: &Value) -> u32 {
    let flags_count: usize = sanitized_payload
        .get("_flags")
        .and_then(Value::as_array)
        .map(|flags| {
            flags
                .iter()
                .map(|entry| {
                    entry
                        .get("count")
                        .and_then(Value::as_u64)
                        .unwrap_or_default() as usize
                })
                .sum()
        })
        .unwrap_or_default();

    (flags_count.saturating_mul(10).min(100)) as u32
}

fn resolve_github_entity_id(payload: &Value) -> String {
    payload
        .get("pull_request")
        .and_then(|pull_request| pull_request.get("number"))
        .or_else(|| payload.get("issue").and_then(|issue| issue.get("number")))
        .or_else(|| payload.get("number"))
        .map(value_to_string)
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_optional_string(path: &[&str], payload: &Value) -> Option<String> {
    let mut current = payload;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(value_to_string(current))
}

fn value_to_string(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(number) = value.as_i64() {
        return number.to_string();
    }
    if let Some(number) = value.as_u64() {
        return number.to_string();
    }
    if let Some(number) = value.as_f64() {
        return number.to_string();
    }
    value.to_string()
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn accepted(reason: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::ACCEPTED,
        Json(json!({"status":"accepted","reason":reason})),
    )
}
