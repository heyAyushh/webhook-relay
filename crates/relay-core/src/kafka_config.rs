use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaCoreConfig {
    pub brokers: Vec<String>,
    pub security_protocol: String,
    pub topic_prefix_core: String,
    pub dlq_topic: String,
    pub producer_defaults: ProducerDefaults,
    pub consumer_defaults: ConsumerDefaults,
    #[serde(default)]
    pub auto_create_topics: Option<bool>,
    #[serde(default)]
    pub topic_partitions: Option<i32>,
    #[serde(default)]
    pub topic_replication_factor: Option<i32>,
    #[serde(default)]
    pub allow_plaintext: Option<bool>,
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub sasl: Option<SaslConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerDefaults {
    pub publish_queue_capacity: usize,
    pub publish_max_retries: u32,
    pub publish_backoff_base_ms: u64,
    pub publish_backoff_max_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerDefaults {
    pub commit_mode: String,
    pub auto_offset_reset: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
    pub ca_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaslConfig {
    pub mechanism: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct KafkaCoreFile {
    kafka_core: KafkaCoreConfig,
}

impl KafkaCoreConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read kafka core config: {}", path.display()))?;
        let parsed = toml::from_str::<KafkaCoreFile>(&raw)
            .with_context(|| format!("parse kafka core config: {}", path.display()))?;
        parsed.kafka_core.validate()?;
        Ok(parsed.kafka_core)
    }

    pub fn from_env() -> Result<Self> {
        let brokers = required_env("KAFKA_BROKERS")?
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        if brokers.is_empty() {
            return Err(anyhow!("KAFKA_BROKERS cannot be empty"));
        }

        let config = Self {
            brokers,
            security_protocol: env::var("KAFKA_SECURITY_PROTOCOL")
                .unwrap_or_else(|_| "ssl".to_string())
                .to_ascii_lowercase(),
            topic_prefix_core: env::var("RELAY_SOURCE_TOPIC_PREFIX")
                .unwrap_or_else(|_| "webhooks".to_string()),
            dlq_topic: env::var("KAFKA_DLQ_TOPIC").unwrap_or_else(|_| "webhooks.dlq".to_string()),
            producer_defaults: ProducerDefaults {
                publish_queue_capacity: env_usize("RELAY_PUBLISH_QUEUE_CAPACITY", 4096)?,
                publish_max_retries: env_u32("RELAY_PUBLISH_MAX_RETRIES", 5)?,
                publish_backoff_base_ms: env_u64("RELAY_PUBLISH_BACKOFF_BASE_MS", 200)?,
                publish_backoff_max_ms: env_u64("RELAY_PUBLISH_BACKOFF_MAX_MS", 5000)?,
            },
            consumer_defaults: ConsumerDefaults {
                commit_mode: "async".to_string(),
                auto_offset_reset: "latest".to_string(),
            },
            auto_create_topics: Some(env_bool("KAFKA_AUTO_CREATE_TOPICS", true)),
            topic_partitions: Some(env_i32("KAFKA_TOPIC_PARTITIONS", 3)?),
            topic_replication_factor: Some(env_i32("KAFKA_TOPIC_REPLICATION_FACTOR", 1)?),
            allow_plaintext: Some(env_bool("KAFKA_ALLOW_PLAINTEXT", false)),
            tls: env_tls_config(),
            sasl: env_sasl_config(),
        };

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.topic_prefix_core.trim().is_empty() {
            return Err(anyhow!("kafka_core.topic_prefix_core cannot be empty"));
        }
        if self.dlq_topic.trim().is_empty() {
            return Err(anyhow!("kafka_core.dlq_topic cannot be empty"));
        }

        let protocol = self.security_protocol.trim().to_ascii_lowercase();
        if protocol == "plaintext" && !self.allow_plaintext.unwrap_or(false) {
            return Err(anyhow!(
                "kafka_core.security_protocol=plaintext requires kafka_core.allow_plaintext=true"
            ));
        }

        if protocol == "ssl" && self.tls.is_none() {
            return Err(anyhow!(
                "kafka_core.tls is required when kafka_core.security_protocol=ssl"
            ));
        }

        Ok(())
    }
}

fn env_tls_config() -> Option<TlsConfig> {
    let cert = env::var("KAFKA_TLS_CERT").ok()?;
    let key = env::var("KAFKA_TLS_KEY").ok()?;
    let ca = env::var("KAFKA_TLS_CA").ok()?;
    Some(TlsConfig {
        cert_path: cert,
        key_path: key,
        ca_path: ca,
    })
}

fn env_sasl_config() -> Option<SaslConfig> {
    let mechanism = env::var("KAFKA_SASL_MECHANISM").ok()?;
    let username = env::var("KAFKA_SASL_USERNAME").ok();
    let password = env::var("KAFKA_SASL_PASSWORD").ok();
    Some(SaslConfig {
        mechanism,
        username,
        password,
    })
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing required env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("{name} cannot be empty"));
    }
    Ok(value)
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

#[cfg(test)]
mod tests {
    use super::KafkaCoreConfig;
    use std::fs;

    #[test]
    fn loads_toml_file() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let file_path = temp_dir.path().join("kafka-core.toml");
        fs::write(
            &file_path,
            r#"
[kafka_core]
brokers = ["127.0.0.1:9092"]
security_protocol = "plaintext"
allow_plaintext = true
topic_prefix_core = "webhooks"
dlq_topic = "webhooks.dlq"

[kafka_core.producer_defaults]
publish_queue_capacity = 10
publish_max_retries = 5
publish_backoff_base_ms = 100
publish_backoff_max_ms = 1000

[kafka_core.consumer_defaults]
commit_mode = "async"
auto_offset_reset = "latest"
"#,
        )
        .expect("write config");

        let parsed = KafkaCoreConfig::load(&file_path).expect("load config");
        assert_eq!(parsed.brokers, vec!["127.0.0.1:9092"]);
        assert_eq!(parsed.topic_prefix_core, "webhooks");
    }
}
