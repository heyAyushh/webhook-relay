use anyhow::{Context, Result, anyhow};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub kafka_brokers: String,
    pub kafka_tls_cert: String,
    pub kafka_tls_key: String,
    pub kafka_tls_ca: String,
    pub kafka_group_id: String,
    pub kafka_topics: Vec<String>,
    pub openclaw_webhook_url: String,
    pub openclaw_webhook_token: String,
    pub openclaw_agent_id: String,
    pub openclaw_session_key: String,
    pub openclaw_wake_mode: String,
    pub openclaw_name: String,
    pub openclaw_deliver: bool,
    pub openclaw_channel: String,
    pub openclaw_to: String,
    pub openclaw_model: String,
    pub openclaw_thinking: String,
    pub openclaw_timeout_seconds: u64,
    pub openclaw_message_max_bytes: usize,
    pub openclaw_http_timeout_seconds: u64,
    pub dlq_topic: String,
    pub max_retries: u32,
    pub backoff_base_seconds: u64,
    pub backoff_max_seconds: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let kafka_topics_raw = env::var("KAFKA_TOPICS")
            .unwrap_or_else(|_| "webhooks.github,webhooks.linear".to_string());
        let kafka_topics = kafka_topics_raw
            .split(',')
            .map(str::trim)
            .filter(|topic| !topic.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        if kafka_topics.is_empty() {
            return Err(anyhow!("KAFKA_TOPICS cannot be empty"));
        }

        let config = Self {
            kafka_brokers: required_env("KAFKA_BROKERS")?,
            kafka_tls_cert: required_env("KAFKA_TLS_CERT")?,
            kafka_tls_key: required_env("KAFKA_TLS_KEY")?,
            kafka_tls_ca: required_env("KAFKA_TLS_CA")?,
            kafka_group_id: env::var("KAFKA_GROUP_ID")
                .unwrap_or_else(|_| "kafka-openclaw-hook".to_string()),
            kafka_topics,
            openclaw_webhook_url: required_env("OPENCLAW_WEBHOOK_URL")?,
            openclaw_webhook_token: required_env("OPENCLAW_WEBHOOK_TOKEN")?,
            openclaw_agent_id: env::var("OPENCLAW_AGENT_ID")
                .unwrap_or_else(|_| "coder".to_string()),
            openclaw_session_key: env::var("OPENCLAW_SESSION_KEY")
                .unwrap_or_else(|_| "coder:orchestrator".to_string()),
            openclaw_wake_mode: env::var("OPENCLAW_WAKE_MODE")
                .unwrap_or_else(|_| "now".to_string()),
            openclaw_name: env::var("OPENCLAW_NAME").unwrap_or_else(|_| "WebhookRelay".to_string()),
            openclaw_deliver: env_bool("OPENCLAW_DELIVER", true),
            openclaw_channel: env::var("OPENCLAW_CHANNEL")
                .unwrap_or_else(|_| "telegram".to_string()),
            openclaw_to: env::var("OPENCLAW_TO")
                .unwrap_or_else(|_| "-1003734912836:topic:2".to_string()),
            openclaw_model: env::var("OPENCLAW_MODEL")
                .unwrap_or_else(|_| "anthropic/claude-sonnet-4-6".to_string()),
            openclaw_thinking: env::var("OPENCLAW_THINKING").unwrap_or_else(|_| "low".to_string()),
            openclaw_timeout_seconds: env_u64("OPENCLAW_TIMEOUT_SECONDS", 600)?,
            openclaw_message_max_bytes: env_usize("OPENCLAW_MESSAGE_MAX_BYTES", 4_000)?,
            openclaw_http_timeout_seconds: env_u64("OPENCLAW_HTTP_TIMEOUT_SECONDS", 20)?,
            dlq_topic: env::var("KAFKA_DLQ_TOPIC").unwrap_or_else(|_| "webhooks.dlq".to_string()),
            max_retries: env_u32("CONSUMER_MAX_RETRIES", 5)?,
            backoff_base_seconds: env_u64("CONSUMER_BACKOFF_BASE_SECONDS", 1)?,
            backoff_max_seconds: env_u64("CONSUMER_BACKOFF_MAX_SECONDS", 30)?,
        };

        if config.openclaw_timeout_seconds == 0 {
            return Err(anyhow!("OPENCLAW_TIMEOUT_SECONDS must be greater than 0"));
        }

        if config.openclaw_message_max_bytes < 128 {
            return Err(anyhow!("OPENCLAW_MESSAGE_MAX_BYTES must be at least 128"));
        }

        if config.openclaw_http_timeout_seconds == 0 {
            return Err(anyhow!(
                "OPENCLAW_HTTP_TIMEOUT_SECONDS must be greater than 0"
            ));
        }

        Ok(config)
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("env var {name} cannot be empty"));
    }
    Ok(value)
}

fn env_u32(name: &str, default: u32) -> Result<u32> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<u32>()
                .with_context(|| format!("invalid u32 for {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_u64(name: &str, default: u64) -> Result<u64> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<u64>()
                .with_context(|| format!("invalid u64 for {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_usize(name: &str, default: usize) -> Result<usize> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid usize for {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}
