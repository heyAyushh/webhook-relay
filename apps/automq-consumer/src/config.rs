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
    pub dlq_topic: String,
    pub max_retries: u32,
    pub backoff_base_seconds: u64,
    pub backoff_max_seconds: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let kafka_topics_raw = required_env("KAFKA_TOPICS")?;
        let kafka_topics = kafka_topics_raw
            .split(',')
            .map(str::trim)
            .filter(|topic| !topic.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        if kafka_topics.is_empty() {
            return Err(anyhow!("KAFKA_TOPICS cannot be empty"));
        }

        Ok(Self {
            kafka_brokers: required_env("KAFKA_BROKERS")?,
            kafka_tls_cert: required_env("KAFKA_TLS_CERT")?,
            kafka_tls_key: required_env("KAFKA_TLS_KEY")?,
            kafka_tls_ca: required_env("KAFKA_TLS_CA")?,
            kafka_group_id: env::var("KAFKA_GROUP_ID")
                .unwrap_or_else(|_| "openclaw-consumer".to_string()),
            kafka_topics,
            openclaw_webhook_url: required_env("OPENCLAW_WEBHOOK_URL")?,
            openclaw_webhook_token: required_env("OPENCLAW_WEBHOOK_TOKEN")?,
            dlq_topic: env::var("KAFKA_DLQ_TOPIC").unwrap_or_else(|_| "webhooks.dlq".to_string()),
            max_retries: env_u32("CONSUMER_MAX_RETRIES", 5)?,
            backoff_base_seconds: env_u64("CONSUMER_BACKOFF_BASE_SECONDS", 1)?,
            backoff_max_seconds: env_u64("CONSUMER_BACKOFF_MAX_SECONDS", 30)?,
        })
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
