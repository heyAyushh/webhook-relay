use anyhow::{Context, Result, anyhow};
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub kafka_brokers: String,
    pub kafka_tls_cert: String,
    pub kafka_tls_key: String,
    pub kafka_tls_ca: String,
    pub kafka_dlq_topic: String,
    pub kafka_auto_create_topics: bool,
    pub kafka_topic_partitions: i32,
    pub kafka_topic_replication_factor: i32,
    pub hmac_secret_github: String,
    pub hmac_secret_linear: String,
    pub max_payload_bytes: usize,
    pub ip_limit_per_minute: u32,
    pub source_limit_per_minute: u32,
    pub publish_queue_capacity: usize,
    pub publish_max_retries: u32,
    pub publish_backoff_base_ms: u64,
    pub publish_backoff_max_ms: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let config = Self {
            bind_addr: env::var("RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            kafka_brokers: required_env("KAFKA_BROKERS")?,
            kafka_tls_cert: required_env("KAFKA_TLS_CERT")?,
            kafka_tls_key: required_env("KAFKA_TLS_KEY")?,
            kafka_tls_ca: required_env("KAFKA_TLS_CA")?,
            kafka_dlq_topic: env::var("KAFKA_DLQ_TOPIC")
                .unwrap_or_else(|_| "webhooks.dlq".to_string()),
            kafka_auto_create_topics: env_bool("KAFKA_AUTO_CREATE_TOPICS", true),
            kafka_topic_partitions: env_i32("KAFKA_TOPIC_PARTITIONS", 3)?,
            kafka_topic_replication_factor: env_i32("KAFKA_TOPIC_REPLICATION_FACTOR", 1)?,
            hmac_secret_github: required_env("HMAC_SECRET_GITHUB")?,
            hmac_secret_linear: required_env("HMAC_SECRET_LINEAR")?,
            max_payload_bytes: env_usize("RELAY_MAX_PAYLOAD_BYTES", 1_048_576)?,
            ip_limit_per_minute: env_u32("RELAY_IP_RATE_PER_MINUTE", 100)?,
            source_limit_per_minute: env_u32("RELAY_SOURCE_RATE_PER_MINUTE", 500)?,
            publish_queue_capacity: env_usize("RELAY_PUBLISH_QUEUE_CAPACITY", 4096)?,
            publish_max_retries: env_u32("RELAY_PUBLISH_MAX_RETRIES", 5)?,
            publish_backoff_base_ms: env_u64("RELAY_PUBLISH_BACKOFF_BASE_MS", 200)?,
            publish_backoff_max_ms: env_u64("RELAY_PUBLISH_BACKOFF_MAX_MS", 5_000)?,
        };

        if config.kafka_topic_partitions <= 0 {
            return Err(anyhow!("KAFKA_TOPIC_PARTITIONS must be a positive integer"));
        }

        if config.kafka_topic_replication_factor <= 0 {
            return Err(anyhow!(
                "KAFKA_TOPIC_REPLICATION_FACTOR must be a positive integer"
            ));
        }

        Ok(config)
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing required env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("required env var {name} cannot be empty"));
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

fn env_i32(name: &str, default: i32) -> Result<i32> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<i32>()
                .with_context(|| format!("invalid i32 for {name}"))
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
