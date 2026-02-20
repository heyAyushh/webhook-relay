use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use relay_core::model::WebhookEnvelope;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use tokio::time::{Duration, sleep};

const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
const MESSAGE_MAX_BYTES: usize = 4_000;
const AGENT_ID: &str = "coder";
const SESSION_KEY: &str = "coder:orchestrator";
const WAKE_MODE: &str = "now";
const WEBHOOK_NAME: &str = "WebhookRelay";
const CHANNEL: &str = "telegram";
const TELEGRAM_TOPIC: &str = "-1003734912836:topic:2";
const MODEL: &str = "anthropic/claude-sonnet-4-6";
const THINKING: &str = "low";
const TIMEOUT_SECONDS: u64 = 600;

#[derive(Clone)]
pub struct Forwarder {
    config: Config,
    client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentWebhookPayload {
    agent_id: &'static str,
    session_key: &'static str,
    wake_mode: &'static str,
    name: &'static str,
    deliver: bool,
    channel: &'static str,
    to: &'static str,
    model: &'static str,
    thinking: &'static str,
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
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS))
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
            agent_id: AGENT_ID,
            session_key: SESSION_KEY,
            wake_mode: WAKE_MODE,
            name: WEBHOOK_NAME,
            deliver: true,
            channel: CHANNEL,
            to: TELEGRAM_TOPIC,
            model: MODEL,
            thinking: THINKING,
            timeout_seconds: TIMEOUT_SECONDS,
            message: build_message(envelope),
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

fn build_message(envelope: &WebhookEnvelope) -> String {
    let payload_summary = summarize_payload(&envelope.payload, MESSAGE_MAX_BYTES);
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

        let message = build_message(&envelope);
        assert!(message.contains("[github] pull_request.opened"));
        assert!(message.contains("Event ID: id-1"));
        assert!(message.contains("\"number\":42"));
    }
}
