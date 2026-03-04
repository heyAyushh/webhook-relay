use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use relay_core::model::WebhookEnvelope;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};
use tracing::{info, warn};

#[derive(Clone)]
pub struct WebsocketServerOutputAdapter {
    sender: broadcast::Sender<String>,
}

#[derive(Clone)]
struct WebsocketServerState {
    sender: broadcast::Sender<String>,
    auth_mode: String,
    auth_token: Option<String>,
    max_clients: usize,
    send_timeout_ms: u64,
    active_clients: Arc<AtomicUsize>,
}

impl WebsocketServerOutputAdapter {
    pub async fn start(
        adapter_id: &str,
        bind: &str,
        path: &str,
        auth_mode: &str,
        auth_token: Option<String>,
        max_clients: usize,
        queue_depth_per_client: usize,
        send_timeout_ms: u64,
    ) -> Result<Self> {
        let (sender, _) = broadcast::channel(queue_depth_per_client);
        let state = WebsocketServerState {
            sender: sender.clone(),
            auth_mode: auth_mode.to_string(),
            auth_token,
            max_clients,
            send_timeout_ms,
            active_clients: Arc::new(AtomicUsize::new(0)),
        };

        let app = Router::new()
            .route(path, get(websocket_server_connect_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind(bind)
            .await
            .with_context(|| format!("bind websocket_server_output {}", bind))?;

        tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app).await {
                warn!(error = %error, "websocket_server_output stopped");
            }
        });
        info!(
            adapter_id = adapter_id,
            bind = bind,
            path = path,
            max_clients = max_clients,
            "websocket_server_output started"
        );

        Ok(Self { sender })
    }

    pub async fn broadcast(&self, envelope: &WebhookEnvelope) -> Result<()> {
        let payload =
            serde_json::to_string(envelope).context("serialize websocket server payload")?;
        self.sender
            .send(payload)
            .map_err(|error| anyhow::anyhow!("websocket server broadcast failed: {}", error))?;
        Ok(())
    }
}

async fn websocket_server_connect_handler(
    State(state): State<WebsocketServerState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if !authorize_request(
        headers,
        state.auth_mode.as_str(),
        state.auth_token.as_deref(),
    ) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }

    if state.active_clients.load(Ordering::SeqCst) >= state.max_clients {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error":"max websocket clients reached"})),
        )
            .into_response();
    }

    ws.on_upgrade(move |socket| websocket_server_client_session(state, socket))
        .into_response()
}

async fn websocket_server_client_session(state: WebsocketServerState, mut socket: WebSocket) {
    state.active_clients.fetch_add(1, Ordering::SeqCst);
    let mut receiver = state.sender.subscribe();

    loop {
        tokio::select! {
            maybe_message = receiver.recv() => {
                match maybe_message {
                    Ok(message) => {
                        let send_result = timeout(
                            Duration::from_millis(state.send_timeout_ms),
                            socket.send(WsMessage::Text(message.into())),
                        ).await;
                        match send_result {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) | Err(_) => break,
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            frame = socket.recv() => {
                match frame {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    state.active_clients.fetch_sub(1, Ordering::SeqCst);
}

fn authorize_request(headers: HeaderMap, auth_mode: &str, expected_token: Option<&str>) -> bool {
    match auth_mode.trim().to_ascii_lowercase().as_str() {
        "none" => true,
        "bearer" | "hmac" => {
            let Some(expected_token) = expected_token else {
                return false;
            };
            let token = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_bearer)
                .or_else(|| {
                    headers
                        .get("x-adapter-token")
                        .and_then(|value| value.to_str().ok())
                        .map(str::trim)
                        .map(ToString::to_string)
                });
            token
                .map(|provided| provided == expected_token)
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn parse_bearer(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let prefix = "bearer ";
    if trimmed.len() < prefix.len() || !trimmed[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let token = trimmed[prefix.len()..].trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
