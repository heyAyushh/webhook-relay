use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use futures_util::StreamExt;
use rdkafka::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::Message;
use relay_core::model::EventMeta;
use relay_core::sanitize::sanitize_payload;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;
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
use uuid::Uuid;
use webhook_relay::client_ip::TrustedClientIpKeyExtractor;
use webhook_relay::config::{
    Config, RuntimeIngressAdapter, RuntimeServePluginConfig, ServeRouteRule,
};
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
    http_ingress_adapter_id: Option<String>,
    http_ingress_plugins: Vec<RuntimeServePluginConfig>,
    websocket_ingress: Option<WebsocketIngressRuntime>,
    mcp_ingress: Option<McpIngressRuntime>,
}

const MAX_RAW_BODY_PREVIEW_CHARS: usize = 4_096;

#[derive(Debug, Clone)]
struct WebsocketIngressRuntime {
    id: String,
    path_template: String,
    auth_mode: String,
    auth_token: Option<String>,
    plugins: Vec<RuntimeServePluginConfig>,
}

#[derive(Debug, Clone)]
struct McpIngressRuntime {
    id: String,
    tool_name: String,
    path: String,
    auth_mode: String,
    auth_token: Option<String>,
    max_payload_bytes: usize,
    plugins: Vec<RuntimeServePluginConfig>,
}

#[derive(Debug, Clone)]
struct KafkaIngressRuntime {
    id: String,
    topics: Vec<String>,
    group_id: String,
    brokers: String,
    plugins: Vec<RuntimeServePluginConfig>,
}

#[derive(Debug, Clone)]
struct IngressRuntimeSelection {
    http_path: String,
    http_ingress_adapter_id: Option<String>,
    http_ingress_plugins: Vec<RuntimeServePluginConfig>,
    websocket_ingress: Option<WebsocketIngressRuntime>,
    mcp_ingress: Option<McpIngressRuntime>,
    kafka_ingress_adapters: Vec<KafkaIngressRuntime>,
}

#[derive(Debug, Deserialize)]
struct McpIngestRequest {
    source: String,
    payload: Value,
    #[serde(default)]
    event_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WsIngressFrame {
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    event_type: Option<String>,
}

#[derive(Debug, Clone)]
struct EnqueueAccepted {
    event_id: String,
    topic: String,
    event_type: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing();

    let config = Config::from_env().context("load relay config")?;
    let ingress_runtime = resolve_ingress_runtime(&config).context("resolve ingress adapters")?;
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
        http_ingress_adapter_id: ingress_runtime.http_ingress_adapter_id.clone(),
        http_ingress_plugins: ingress_runtime.http_ingress_plugins.clone(),
        websocket_ingress: ingress_runtime.websocket_ingress.clone(),
        mcp_ingress: ingress_runtime.mcp_ingress.clone(),
    });

    for kafka_ingress in ingress_runtime.kafka_ingress_adapters {
        let state_for_worker = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run_kafka_ingress_worker(state_for_worker, kafka_ingress).await {
                warn!(error = %error, "kafka ingress worker exited");
            }
        });
    }

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

    let mut app = Router::new()
        .route(ingress_runtime.http_path.as_str(), post(webhook_handler))
        .route("/health", get(health))
        .route("/ready", get(ready));
    if let Some(websocket_ingress) = ingress_runtime.websocket_ingress.as_ref() {
        app = app.route(
            websocket_ingress.path_template.as_str(),
            get(websocket_ingress_handler),
        );
    }
    if let Some(mcp_ingress) = ingress_runtime.mcp_ingress.as_ref() {
        app = app.route(mcp_ingress.path.as_str(), post(mcp_ingest_handler));
    }
    let app = app
        .layer(DefaultBodyLimit::max(state.config.max_payload_bytes))
        .layer(GovernorLayer::new(governor_config))
        .with_state(state.clone());

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .with_context(|| format!("bind {}", state.config.bind_addr))?;

    info!(
        bind = %state.config.bind_addr,
        http_path = ingress_runtime.http_path.as_str(),
        websocket_ingress_path = ingress_runtime
            .websocket_ingress
            .as_ref()
            .map(|adapter| adapter.path_template.as_str()),
        mcp_ingress_path = ingress_runtime
            .mcp_ingress
            .as_ref()
            .map(|adapter| adapter.path.as_str()),
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

    let (event_type, sanitized_payload, plugin_flags) =
        match apply_serve_plugins(&state.http_ingress_plugins, event_type, sanitized_payload) {
            Ok(output) => output,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": error.to_string()})),
                );
            }
        };

    let matched_route = match resolve_serve_route(&state.config, source, event_type.as_str()) {
        Some(route) => Some(route),
        None if state.config.serve_routes.is_empty() => None,
        None => {
            warn!(
                source,
                event_type = event_type.as_str(),
                "no matching serve route for inbound event"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"no matching serve route"})),
            );
        }
    };
    let route_key = matched_route.map(|route| route.id.clone());
    let topic = matched_route
        .map(|route| route.target_topic.clone())
        .unwrap_or_else(|| handler.topic_name(&state.config));

    let trace_id = if route_key.is_some() || state.http_ingress_adapter_id.is_some() {
        Some(Uuid::new_v4().to_string())
    } else {
        None
    };
    let event_meta = build_event_meta(
        trace_id.clone(),
        state.http_ingress_adapter_id.clone(),
        route_key.clone(),
        plugin_flags,
    );
    let envelope = build_envelope(source, event_type, sanitized_payload, event_meta);
    debug!(
        source,
        topic = topic.as_str(),
        route_key = ?route_key,
        trace_id = ?trace_id,
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
                route_key = ?route_key,
                trace_id = ?trace_id,
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

async fn websocket_ingress_handler(
    State(state): State<Arc<AppState>>,
    Path(source_path): Path<String>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(adapter) = state.websocket_ingress.clone() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"}))).into_response();
    };
    if !authorize_adapter_request(
        &headers,
        adapter.auth_mode.as_str(),
        adapter.auth_token.as_deref(),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }

    websocket
        .on_upgrade(move |socket| {
            run_websocket_ingress_session(state, socket, source_path, adapter)
        })
        .into_response()
}

async fn run_websocket_ingress_session(
    state: Arc<AppState>,
    mut socket: WebSocket,
    source: String,
    adapter: WebsocketIngressRuntime,
) {
    while let Some(frame_result) = socket.next().await {
        match frame_result {
            Ok(WsMessage::Text(text)) => {
                let parsed_frame = parse_ws_frame_payload(text.as_ref());
                let response = match parsed_frame {
                    Ok((payload, event_type)) => match enqueue_prevalidated_event(
                        &state,
                        source.as_str(),
                        payload,
                        event_type,
                        Some(adapter.id.clone()),
                        &adapter.plugins,
                    )
                    .await
                    {
                        Ok(accepted) => json!({
                            "status": "ok",
                            "event_id": accepted.event_id,
                            "kafka_topic": accepted.topic,
                        }),
                        Err(error) => json!({
                            "status": "error",
                            "message": error.to_string(),
                        }),
                    },
                    Err(error) => json!({
                        "status": "error",
                        "message": error.to_string(),
                    }),
                };
                if socket
                    .send(WsMessage::Text(response.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(WsMessage::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

async fn mcp_ingest_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<McpIngestRequest>,
) -> impl IntoResponse {
    let Some(adapter) = state.mcp_ingress.clone() else {
        return (StatusCode::NOT_FOUND, Json(json!({"error":"not found"})));
    };
    if !authorize_adapter_request(
        &headers,
        adapter.auth_mode.as_str(),
        adapter.auth_token.as_deref(),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }

    let payload_bytes = request.payload.to_string().len();
    if payload_bytes > adapter.max_payload_bytes {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error":"payload exceeds adapter max_payload_bytes"})),
        );
    }

    let accepted = match enqueue_prevalidated_event(
        &state,
        request.source.as_str(),
        request.payload,
        request.event_type,
        Some(adapter.id),
        &adapter.plugins,
    )
    .await
    {
        Ok(accepted) => accepted,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": error.to_string()})),
            );
        }
    };

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "event_id": accepted.event_id,
            "source": request.source,
            "event_type": accepted.event_type,
            "kafka_topic": accepted.topic,
            "queued_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            "tool_name": adapter.tool_name,
        })),
    )
}

async fn enqueue_prevalidated_event(
    state: &Arc<AppState>,
    source: &str,
    payload: Value,
    event_type_override: Option<String>,
    ingress_adapter_id: Option<String>,
    plugins: &[RuntimeServePluginConfig],
) -> Result<EnqueueAccepted> {
    let Some(normalized_source) = normalize_source_name(source) else {
        return Err(anyhow::anyhow!("source cannot be empty"));
    };
    if !state.config.is_source_enabled(&normalized_source) {
        return Err(anyhow::anyhow!(
            "source '{}' is not enabled",
            normalized_source
        ));
    }

    let event_type = if let Some(value) = event_type_override {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(anyhow::anyhow!("event_type override cannot be empty"));
        }
        trimmed.to_string()
    } else if let Some(handler) = handler_for_source(&normalized_source) {
        handler
            .event_type(&HeaderMap::new(), &payload)
            .map_err(|error| anyhow::anyhow!("derive event_type failed: {:?}", error))?
    } else {
        "event".to_string()
    };

    let sanitized_payload = sanitize_payload(&normalized_source, &payload)
        .map_err(|error| anyhow::anyhow!("payload sanitizer rejected request: {}", error))?;
    let (event_type, sanitized_payload, plugin_flags) =
        apply_serve_plugins(plugins, event_type, sanitized_payload)?;
    let matched_route = resolve_serve_route(&state.config, &normalized_source, event_type.as_str());
    let route_key = matched_route.map(|route| route.id.clone());
    let topic = matched_route
        .map(|route| route.target_topic.clone())
        .unwrap_or_else(|| state.config.source_topic_name(&normalized_source));
    let trace_id = Some(Uuid::new_v4().to_string());
    let event_meta = build_event_meta(
        trace_id.clone(),
        ingress_adapter_id.clone(),
        route_key.clone(),
        plugin_flags,
    );
    let envelope = build_envelope(
        &normalized_source,
        event_type.clone(),
        sanitized_payload,
        event_meta,
    );
    let event_id = envelope.id.clone();
    state
        .publish_tx
        .try_send(PublishJob {
            topic: topic.clone(),
            envelope,
        })
        .map_err(|error| anyhow::anyhow!("failed to enqueue event: {}", error))?;

    Ok(EnqueueAccepted {
        event_id,
        topic,
        event_type,
    })
}

async fn run_kafka_ingress_worker(
    state: Arc<AppState>,
    adapter: KafkaIngressRuntime,
) -> Result<()> {
    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", &adapter.brokers)
        .set("group.id", &adapter.group_id)
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "latest")
        .set("security.protocol", &state.config.kafka_security_protocol);
    if state.config.kafka_security_protocol == "ssl" {
        client_config
            .set("ssl.certificate.location", &state.config.kafka_tls_cert)
            .set("ssl.key.location", &state.config.kafka_tls_key)
            .set("ssl.ca.location", &state.config.kafka_tls_ca);
    }

    let consumer = client_config
        .create::<StreamConsumer>()
        .context("create kafka ingress consumer")?;
    let topic_refs = adapter
        .topics
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    consumer
        .subscribe(&topic_refs)
        .with_context(|| format!("subscribe kafka ingress topics: {}", topic_refs.join(",")))?;
    info!(
        adapter_id = adapter.id.as_str(),
        topics = ?adapter.topics,
        group_id = adapter.group_id.as_str(),
        "kafka ingress worker started"
    );

    loop {
        let message = match consumer.recv().await {
            Ok(message) => message,
            Err(error) => {
                warn!(
                    adapter_id = adapter.id.as_str(),
                    error = %error,
                    "kafka ingress receive error"
                );
                continue;
            }
        };

        let payload_bytes = match message.payload() {
            Some(payload) => payload,
            None => {
                warn!(
                    adapter_id = adapter.id.as_str(),
                    topic = message.topic(),
                    "kafka ingress message missing payload"
                );
                let _ = consumer.commit_message(&message, CommitMode::Async);
                continue;
            }
        };

        let parsed = parse_kafka_ingress_payload(payload_bytes);
        if let Err(error) = &parsed {
            warn!(
                adapter_id = adapter.id.as_str(),
                topic = message.topic(),
                error = %error,
                "kafka ingress payload parse failed"
            );
        }
        if let Ok((source, payload, event_type)) = parsed {
            if let Err(error) = enqueue_prevalidated_event(
                &state,
                source.as_str(),
                payload,
                event_type,
                Some(adapter.id.clone()),
                &adapter.plugins,
            )
            .await
            {
                warn!(
                    adapter_id = adapter.id.as_str(),
                    topic = message.topic(),
                    error = %error,
                    "kafka ingress enqueue failed"
                );
            }
        }

        if let Err(error) = consumer.commit_message(&message, CommitMode::Async) {
            warn!(
                adapter_id = adapter.id.as_str(),
                topic = message.topic(),
                error = %error,
                "kafka ingress commit failed"
            );
        }
    }
}

fn parse_kafka_ingress_payload(payload: &[u8]) -> Result<(String, Value, Option<String>)> {
    #[derive(Debug, Deserialize)]
    struct ParsedKafkaIngress {
        source: String,
        payload: Value,
        #[serde(default)]
        event_type: Option<String>,
    }

    if let Ok(envelope) = serde_json::from_slice::<relay_core::model::EventEnvelope>(payload) {
        return Ok((envelope.source, envelope.payload, Some(envelope.event_type)));
    }

    let parsed = serde_json::from_slice::<ParsedKafkaIngress>(payload)
        .context("parse kafka ingress payload as object")?;
    Ok((parsed.source, parsed.payload, parsed.event_type))
}

fn parse_ws_frame_payload(raw: &str) -> Result<(Value, Option<String>)> {
    let parsed: Value = serde_json::from_str(raw).context("parse websocket frame JSON")?;
    let frame = serde_json::from_value::<WsIngressFrame>(parsed.clone()).ok();
    match frame {
        Some(frame) => Ok((frame.payload.unwrap_or(parsed), frame.event_type)),
        None => Ok((parsed, None)),
    }
}

fn resolve_ingress_runtime(config: &Config) -> Result<IngressRuntimeSelection> {
    let mut http_path = "/webhook/{source}".to_string();
    let mut http_ingress_adapter_id = config.active_ingress_adapter_id.clone();
    let mut http_ingress_plugins = Vec::new();
    let mut websocket_ingress = None;
    let mut mcp_ingress = None;
    let mut kafka_ingress_adapters = Vec::new();

    for adapter in &config.ingress_adapters {
        match adapter {
            RuntimeIngressAdapter::HttpWebhookIngress {
                id,
                bind,
                path_template,
                plugins,
            } => {
                if bind.trim() != config.bind_addr.trim() {
                    warn!(
                        adapter_id = id.as_str(),
                        adapter_bind = bind.as_str(),
                        relay_bind = config.bind_addr.as_str(),
                        "http_webhook_ingress bind differs from relay bind; relay bind takes precedence"
                    );
                }
                if http_ingress_adapter_id.is_none() {
                    http_ingress_adapter_id = Some(id.clone());
                }
                http_path = path_template.clone();
                http_ingress_plugins = plugins.clone();
            }
            RuntimeIngressAdapter::WebsocketIngress {
                id,
                path_template,
                auth_mode,
                token_env,
                plugins,
            } => {
                let auth_token =
                    resolve_auth_token(auth_mode.as_str(), token_env.as_deref(), id.as_str())?;
                websocket_ingress = Some(WebsocketIngressRuntime {
                    id: id.clone(),
                    path_template: path_template.clone(),
                    auth_mode: auth_mode.clone(),
                    auth_token,
                    plugins: plugins.clone(),
                });
            }
            RuntimeIngressAdapter::McpIngestExposed {
                id,
                tool_name,
                bind,
                path,
                auth_mode,
                token_env,
                max_payload_bytes,
                plugins,
                ..
            } => {
                if bind.trim() != config.bind_addr.trim() {
                    warn!(
                        adapter_id = id.as_str(),
                        adapter_bind = bind.as_str(),
                        relay_bind = config.bind_addr.as_str(),
                        "mcp_ingest_exposed bind differs from relay bind; relay bind takes precedence"
                    );
                }
                let auth_token =
                    resolve_auth_token(auth_mode.as_str(), token_env.as_deref(), id.as_str())?;
                mcp_ingress = Some(McpIngressRuntime {
                    id: id.clone(),
                    tool_name: tool_name.clone(),
                    path: path.clone(),
                    auth_mode: auth_mode.clone(),
                    auth_token,
                    max_payload_bytes: *max_payload_bytes,
                    plugins: plugins.clone(),
                });
            }
            RuntimeIngressAdapter::KafkaIngress {
                id,
                topics,
                group_id,
                brokers,
                plugins,
            } => kafka_ingress_adapters.push(KafkaIngressRuntime {
                id: id.clone(),
                topics: topics.clone(),
                group_id: group_id.clone(),
                brokers: brokers
                    .clone()
                    .unwrap_or_else(|| config.kafka_brokers.clone()),
                plugins: plugins.clone(),
            }),
        }
    }

    Ok(IngressRuntimeSelection {
        http_path,
        http_ingress_adapter_id,
        http_ingress_plugins,
        websocket_ingress,
        mcp_ingress,
        kafka_ingress_adapters,
    })
}

fn resolve_auth_token(
    auth_mode: &str,
    token_env: Option<&str>,
    adapter_id: &str,
) -> Result<Option<String>> {
    match auth_mode.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(None),
        "bearer" | "hmac" => {
            let token_env = token_env.ok_or_else(|| {
                anyhow::anyhow!(
                    "adapter '{}' auth_mode={} requires token_env",
                    adapter_id,
                    auth_mode
                )
            })?;
            let token = env::var(token_env).with_context(|| {
                format!(
                    "missing auth token env '{}' for adapter '{}'",
                    token_env, adapter_id
                )
            })?;
            if token.trim().is_empty() {
                return Err(anyhow::anyhow!(
                    "auth token env '{}' for adapter '{}' cannot be empty",
                    token_env,
                    adapter_id
                ));
            }
            Ok(Some(token))
        }
        other => Err(anyhow::anyhow!(
            "unsupported auth_mode '{}' for adapter '{}'",
            other,
            adapter_id
        )),
    }
}

fn authorize_adapter_request(
    headers: &HeaderMap,
    auth_mode: &str,
    expected_token: Option<&str>,
) -> bool {
    match auth_mode.trim().to_ascii_lowercase().as_str() {
        "none" => true,
        "bearer" | "hmac" => {
            let Some(expected_token) = expected_token else {
                return false;
            };
            let provided = extract_bearer_token(headers)
                .or_else(|| header_value(headers, "x-adapter-token"))
                .unwrap_or_default();
            provided == expected_token
        }
        _ => false,
    }
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())?
        .trim()
        .to_string();
    let prefix = "bearer ";
    if authorization.len() < prefix.len()
        || !authorization[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        return None;
    }
    let token = authorization[prefix.len()..].trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn header_value(headers: &HeaderMap, key: &str) -> Option<String> {
    headers
        .get(key)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn apply_serve_plugins(
    plugins: &[RuntimeServePluginConfig],
    mut event_type: String,
    payload: Value,
) -> Result<(String, Value, Vec<String>)> {
    let mut flags = Vec::new();

    for plugin in plugins {
        match plugin {
            RuntimeServePluginConfig::EventTypeAlias { from, to } => {
                if event_type == from.as_str() {
                    event_type = to.clone();
                }
            }
            RuntimeServePluginConfig::RequirePayloadField { pointer } => {
                if payload.pointer(pointer).is_none() {
                    return Err(anyhow::anyhow!(
                        "payload missing required field '{}'",
                        pointer
                    ));
                }
            }
            RuntimeServePluginConfig::AddMetaFlag { flag } => {
                if !flags.iter().any(|existing| existing == flag) {
                    flags.push(flag.clone());
                }
            }
        }
    }

    Ok((event_type, payload, flags))
}

fn build_event_meta(
    trace_id: Option<String>,
    ingress_adapter: Option<String>,
    route_key: Option<String>,
    flags: Vec<String>,
) -> Option<EventMeta> {
    if trace_id.is_none() && ingress_adapter.is_none() && route_key.is_none() && flags.is_empty() {
        return None;
    }

    Some(EventMeta {
        trace_id,
        ingress_adapter,
        route_key,
        flags,
    })
}

fn resolve_serve_route<'a>(
    config: &'a Config,
    source: &str,
    event_type: &str,
) -> Option<&'a ServeRouteRule> {
    config.serve_routes.iter().find(|route| {
        wildcard_matches(route.source_match.as_str(), source)
            && wildcard_matches(route.event_type_pattern.as_str(), event_type)
    })
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let normalized_pattern = pattern.trim();
    if normalized_pattern.is_empty() {
        return false;
    }
    if normalized_pattern == "*" {
        return true;
    }
    if !normalized_pattern.contains('*') {
        return normalized_pattern == value;
    }

    let mut remainder = value;
    let requires_prefix = !normalized_pattern.starts_with('*');
    let requires_suffix = !normalized_pattern.ends_with('*');
    let segments = normalized_pattern
        .split('*')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return true;
    }

    for (index, segment) in segments.iter().enumerate() {
        if index == 0 && requires_prefix {
            if !remainder.starts_with(segment) {
                return false;
            }
            remainder = &remainder[segment.len()..];
            continue;
        }

        if index == segments.len() - 1 && requires_suffix {
            return remainder.ends_with(segment);
        }

        match remainder.find(segment) {
            Some(position) => {
                let next_index = position + segment.len();
                remainder = &remainder[next_index..];
            }
            None => return false,
        }
    }

    true
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
            "version": env!("CARGO_PKG_VERSION"),
            "validation_mode": state.config.validation_mode,
            "profile": state.config.active_profile,
            "contract_path": state.config.contract_path,
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
    use super::{apply_serve_plugins, build_event_meta, ip_refill_period_ms, wildcard_matches};
    use relay_core::model::EventMeta;
    use webhook_relay::config::RuntimeServePluginConfig;

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

    #[test]
    fn wildcard_matches_exact_and_glob_patterns() {
        assert!(wildcard_matches("github", "github"));
        assert!(wildcard_matches("*", "github"));
        assert!(wildcard_matches("pull_request.*", "pull_request.opened"));
        assert!(wildcard_matches("*.opened", "pull_request.opened"));
        assert!(!wildcard_matches("linear", "github"));
        assert!(!wildcard_matches("pull_request.*", "issues.opened"));
    }

    #[test]
    fn build_event_meta_returns_none_without_values() {
        assert_eq!(build_event_meta(None, None, None, Vec::new()), None);
    }

    #[test]
    fn build_event_meta_includes_trace_and_route() {
        let meta = build_event_meta(
            Some("trace-1".to_string()),
            Some("http-ingress".to_string()),
            Some("all-to-core".to_string()),
            vec!["plugin.tag".to_string()],
        )
        .expect("meta");
        assert_eq!(
            meta,
            EventMeta {
                trace_id: Some("trace-1".to_string()),
                ingress_adapter: Some("http-ingress".to_string()),
                route_key: Some("all-to-core".to_string()),
                flags: vec!["plugin.tag".to_string()],
            }
        );
    }

    #[test]
    fn apply_serve_plugins_alias_and_flag() {
        let plugins = vec![
            RuntimeServePluginConfig::EventTypeAlias {
                from: "pull_request.opened".to_string(),
                to: "pr.opened".to_string(),
            },
            RuntimeServePluginConfig::AddMetaFlag {
                flag: "serve.plugin.alias".to_string(),
            },
        ];

        let (event_type, payload, flags) = apply_serve_plugins(
            &plugins,
            "pull_request.opened".to_string(),
            serde_json::json!({"action":"opened"}),
        )
        .expect("apply plugins");

        assert_eq!(event_type, "pr.opened");
        assert_eq!(payload["action"].as_str(), Some("opened"));
        assert_eq!(flags, vec!["serve.plugin.alias".to_string()]);
    }

    #[test]
    fn apply_serve_plugins_require_payload_field_fails_closed() {
        let plugins = vec![RuntimeServePluginConfig::RequirePayloadField {
            pointer: "/action".to_string(),
        }];

        let error = apply_serve_plugins(
            &plugins,
            "pull_request.opened".to_string(),
            serde_json::json!({}),
        )
        .expect_err("missing pointer should fail");
        assert!(error.to_string().contains("/action"));
    }
}
