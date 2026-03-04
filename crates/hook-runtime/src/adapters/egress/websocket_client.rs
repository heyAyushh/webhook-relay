use anyhow::{Context, Result, anyhow};
use futures_util::SinkExt;
use relay_core::model::WebhookEnvelope;
use tokio::time::{Duration, sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TungsteniteMessage;
use tokio_tungstenite::tungstenite::http::Request;
use tracing::warn;

#[derive(Clone)]
pub struct WebsocketClientOutputAdapter {
    url: String,
    auth_mode: String,
    auth_token: Option<String>,
    send_timeout_ms: u64,
    retry_max_retries: u32,
    retry_backoff_ms: u64,
}

impl WebsocketClientOutputAdapter {
    pub fn new(
        url: String,
        auth_mode: String,
        auth_token: Option<String>,
        send_timeout_ms: u64,
        retry_max_retries: u32,
        retry_backoff_ms: u64,
    ) -> Self {
        Self {
            url,
            auth_mode,
            auth_token,
            send_timeout_ms,
            retry_max_retries,
            retry_backoff_ms,
        }
    }

    pub async fn send(&self, envelope: &WebhookEnvelope) -> Result<()> {
        let payload = serde_json::to_string(envelope).context("serialize websocket payload")?;
        let attempts = self.retry_max_retries.max(1);
        for attempt in 1..=attempts {
            let result = self.send_once(payload.as_str()).await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) if attempt < attempts => {
                    warn!(
                        attempt,
                        max_attempts = attempts,
                        error = %error,
                        "websocket_client_output send failed; retrying"
                    );
                    sleep(Duration::from_millis(self.retry_backoff_ms)).await;
                }
                Err(error) => return Err(error),
            }
        }

        Err(anyhow!(
            "websocket_client_output retry loop terminated unexpectedly"
        ))
    }

    async fn send_once(&self, payload: &str) -> Result<()> {
        let request = build_websocket_request(
            self.url.as_str(),
            self.auth_mode.as_str(),
            self.auth_token.as_deref(),
        )?;
        let (mut socket, _) = connect_async(request)
            .await
            .context("connect websocket client output")?;

        timeout(
            Duration::from_millis(self.send_timeout_ms),
            socket.send(TungsteniteMessage::Text(payload.to_string().into())),
        )
        .await
        .context("websocket client send timeout")??;

        let _ = socket.close(None).await;
        Ok(())
    }
}

fn build_websocket_request(
    url: &str,
    auth_mode: &str,
    auth_token: Option<&str>,
) -> Result<Request<()>> {
    let mut builder = Request::builder().uri(url);
    if auth_mode != "none" {
        let token = auth_token.ok_or_else(|| anyhow!("websocket auth token missing"))?;
        builder = builder.header("Authorization", format!("Bearer {}", token));
    }
    builder
        .body(())
        .map_err(|error| anyhow!("build websocket request failed: {}", error))
}
