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

/// Mapped-hook payload: OpenClaw `hooks.mappings` provides agent/session/model
/// routing. Consumer only forwards event envelope fields.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MappedHookPayload {
    source: String,
    event_type: String,
    id: String,
    received_at: String,
    payload: String,
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
        let payload = MappedHookPayload {
            source: envelope.source.clone(),
            event_type: envelope.event_type.clone(),
            id: envelope.id.clone(),
            received_at: envelope.received_at.clone(),
            payload: summarize_payload(&envelope.payload, self.config.openclaw_message_max_bytes),
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
    fn summarize_payload_within_limit() {
        let payload = json!({"number":42});
        let summary = summarize_payload(&payload, 4_000);
        assert_eq!(summary, "{\"number\":42}");
    }

    #[test]
    fn summarize_payload_truncates() {
        let payload = json!({"long_key": "a]bbbcccdddeee"});
        let summary = summarize_payload(&payload, 20);
        assert!(summary.ends_with("..."));
        assert!(summary.len() <= 20);
    }
}
