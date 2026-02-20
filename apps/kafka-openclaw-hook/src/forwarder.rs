use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use relay_core::model::WebhookEnvelope;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::time::{Duration, sleep};

#[derive(Clone)]
pub struct Forwarder {
    config: Config,
    client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentWebhookPayload {
    agent_id: String,
    session_key: String,
    wake_mode: String,
    name: String,
    deliver: bool,
    channel: String,
    to: String,
    model: String,
    thinking: String,
    timeout_seconds: u64,
    message: String,
}

#[derive(Debug)]
enum ForwardErrorKind {
    Retryable(String),
    Permanent(String),
}

impl Forwarder {
    pub fn new(config: Config) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.openclaw_http_timeout_seconds))
            .build()
            .context("build reqwest client")?;

        Ok(Self { config, client })
    }

    pub async fn forward_with_retry(&self, envelope: &WebhookEnvelope) -> Result<()> {
        for attempt in 1..=self.config.max_retries {
            match self.forward_once(envelope).await {
                Ok(()) => return Ok(()),
                Err(ForwardErrorKind::Permanent(message)) => {
                    return Err(anyhow!("forward failed permanently: {message}"));
                }
                Err(ForwardErrorKind::Retryable(message)) => {
                    if attempt >= self.config.max_retries {
                        return Err(anyhow!(
                            "forward failed after {} attempts: {}",
                            attempt,
                            message
                        ));
                    }

                    let backoff_seconds = retry_backoff_seconds(
                        self.config.backoff_base_seconds,
                        self.config.backoff_max_seconds,
                        attempt.saturating_sub(1),
                    );
                    sleep(Duration::from_secs(backoff_seconds)).await;
                }
            }
        }

        Err(anyhow!("retry loop terminated unexpectedly"))
    }

    async fn forward_once(
        &self,
        envelope: &WebhookEnvelope,
    ) -> std::result::Result<(), ForwardErrorKind> {
        let payload = AgentWebhookPayload {
            agent_id: self.config.openclaw_agent_id.clone(),
            session_key: self.config.openclaw_session_key.clone(),
            wake_mode: self.config.openclaw_wake_mode.clone(),
            name: self.config.openclaw_name.clone(),
            deliver: self.config.openclaw_deliver,
            channel: self.config.openclaw_channel.clone(),
            to: self.config.openclaw_to.clone(),
            model: self.config.openclaw_model.clone(),
            thinking: self.config.openclaw_thinking.clone(),
            timeout_seconds: self.config.openclaw_timeout_seconds,
            message: build_message(envelope, self.config.openclaw_message_max_bytes),
        };

        let response = match self
            .client
            .post(&self.config.openclaw_webhook_url)
            .header(
                "Authorization",
                format!("Bearer {}", self.config.openclaw_webhook_token),
            )
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                if error.is_timeout() || error.is_connect() || error.is_request() {
                    return Err(ForwardErrorKind::Retryable(error.to_string()));
                }
                return Err(ForwardErrorKind::Permanent(error.to_string()));
            }
        };

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        if status.is_server_error() || status.as_u16() == 429 {
            return Err(ForwardErrorKind::Retryable(format!(
                "OpenClaw returned {status}"
            )));
        }

        Err(ForwardErrorKind::Permanent(format!(
            "OpenClaw returned {status}"
        )))
    }
}

fn build_message(envelope: &WebhookEnvelope, message_max_bytes: usize) -> String {
    let payload_summary = summarize_payload(&envelope.payload, message_max_bytes);
    format!(
        "[{}] {}\nEvent ID: {}\n\n{}",
        envelope.source, envelope.event_type, envelope.id, payload_summary
    )
}

fn summarize_payload(payload: &Value, limit_bytes: usize) -> String {
    let serialized = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    if serialized.len() <= limit_bytes {
        return serialized;
    }

    let mut output = String::new();
    for character in serialized.chars() {
        if output.len() + character.len_utf8() > limit_bytes.saturating_sub(3) {
            break;
        }
        output.push(character);
    }
    output.push_str("...");
    output
}

pub fn retry_backoff_seconds(base_seconds: u64, max_seconds: u64, attempt_index: u32) -> u64 {
    let exponent = attempt_index.min(31);
    let scaled = base_seconds.saturating_mul(1u64 << exponent);
    scaled.min(max_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_backoff_scales_and_caps() {
        assert_eq!(retry_backoff_seconds(1, 30, 0), 1);
        assert_eq!(retry_backoff_seconds(1, 30, 1), 2);
        assert_eq!(retry_backoff_seconds(1, 30, 2), 4);
        assert_eq!(retry_backoff_seconds(1, 30, 3), 8);
        assert_eq!(retry_backoff_seconds(1, 30, 4), 16);
        assert_eq!(retry_backoff_seconds(1, 30, 5), 30);
    }

    #[test]
    fn message_contains_source_event_and_id() {
        let envelope = WebhookEnvelope {
            id: "id-1".to_string(),
            source: "github".to_string(),
            event_type: "pull_request.opened".to_string(),
            received_at: "2026-02-20T14:00:00Z".to_string(),
            payload: json!({"number":42}),
        };

        let message = build_message(&envelope, 4_000);
        assert!(message.contains("[github] pull_request.opened"));
        assert!(message.contains("Event ID: id-1"));
        assert!(message.contains("\"number\":42"));
    }
}
