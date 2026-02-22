use anyhow::{Context, Result, anyhow};
use ipnet::IpNet;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub kafka_brokers: String,
    pub kafka_security_protocol: String,
    pub kafka_allow_plaintext: bool,
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
    pub trust_proxy_headers: bool,
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub dedup_ttl_seconds: i64,
    pub cooldown_seconds: i64,
    pub enforce_linear_timestamp_window: bool,
    pub linear_timestamp_window_seconds: i64,
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
            kafka_security_protocol: env::var("KAFKA_SECURITY_PROTOCOL")
                .unwrap_or_else(|_| "ssl".to_string())
                .trim()
                .to_ascii_lowercase(),
            kafka_allow_plaintext: env_bool("KAFKA_ALLOW_PLAINTEXT", false),
            kafka_tls_cert: env::var("KAFKA_TLS_CERT").unwrap_or_default(),
            kafka_tls_key: env::var("KAFKA_TLS_KEY").unwrap_or_default(),
            kafka_tls_ca: env::var("KAFKA_TLS_CA").unwrap_or_default(),
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
            trust_proxy_headers: env_bool("RELAY_TRUST_PROXY_HEADERS", false),
            trusted_proxy_cidrs: env_cidrs("RELAY_TRUSTED_PROXY_CIDRS", "127.0.0.1/32,::1/128")?,
            dedup_ttl_seconds: env_i64("RELAY_DEDUP_TTL_SECONDS", 604_800)?,
            cooldown_seconds: env_i64("RELAY_COOLDOWN_SECONDS", 30)?,
            enforce_linear_timestamp_window: env_bool(
                "RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW",
                true,
            ),
            linear_timestamp_window_seconds: env_i64("RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS", 60)?,
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

        if config.dedup_ttl_seconds <= 0 {
            return Err(anyhow!(
                "RELAY_DEDUP_TTL_SECONDS must be a positive integer"
            ));
        }

        if config.cooldown_seconds <= 0 {
            return Err(anyhow!("RELAY_COOLDOWN_SECONDS must be a positive integer"));
        }

        if config.linear_timestamp_window_seconds <= 0 {
            return Err(anyhow!(
                "RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS must be a positive integer"
            ));
        }

        if config.trust_proxy_headers && config.trusted_proxy_cidrs.is_empty() {
            return Err(anyhow!(
                "RELAY_TRUSTED_PROXY_CIDRS cannot be empty when RELAY_TRUST_PROXY_HEADERS is enabled"
            ));
        }

        match config.kafka_security_protocol.as_str() {
            "ssl" => {
                if config.kafka_tls_cert.trim().is_empty() {
                    return Err(anyhow!(
                        "KAFKA_TLS_CERT is required when KAFKA_SECURITY_PROTOCOL=ssl"
                    ));
                }
                if config.kafka_tls_key.trim().is_empty() {
                    return Err(anyhow!(
                        "KAFKA_TLS_KEY is required when KAFKA_SECURITY_PROTOCOL=ssl"
                    ));
                }
                if config.kafka_tls_ca.trim().is_empty() {
                    return Err(anyhow!(
                        "KAFKA_TLS_CA is required when KAFKA_SECURITY_PROTOCOL=ssl"
                    ));
                }
            }
            "plaintext" => {
                if !config.kafka_allow_plaintext {
                    return Err(anyhow!(
                        "KAFKA_SECURITY_PROTOCOL=plaintext requires KAFKA_ALLOW_PLAINTEXT=true"
                    ));
                }
            }
            other => {
                return Err(anyhow!(
                    "unsupported KAFKA_SECURITY_PROTOCOL={other}; expected ssl or plaintext"
                ));
            }
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

fn env_i64(name: &str, default: i64) -> Result<i64> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| {
            value
                .parse::<i64>()
                .with_context(|| format!("invalid i64 for {name}"))
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

fn env_cidrs(name: &str, default: &str) -> Result<Vec<IpNet>> {
    let raw = env::var(name).unwrap_or_else(|_| default.to_string());
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<IpNet>()
                .with_context(|| format!("invalid CIDR for {name}: {value}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::env;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    const CONFIG_KEYS: &[&str] = &[
        "RELAY_BIND",
        "KAFKA_BROKERS",
        "KAFKA_SECURITY_PROTOCOL",
        "KAFKA_ALLOW_PLAINTEXT",
        "KAFKA_TLS_CERT",
        "KAFKA_TLS_KEY",
        "KAFKA_TLS_CA",
        "KAFKA_DLQ_TOPIC",
        "KAFKA_AUTO_CREATE_TOPICS",
        "KAFKA_TOPIC_PARTITIONS",
        "KAFKA_TOPIC_REPLICATION_FACTOR",
        "HMAC_SECRET_GITHUB",
        "HMAC_SECRET_LINEAR",
        "RELAY_MAX_PAYLOAD_BYTES",
        "RELAY_IP_RATE_PER_MINUTE",
        "RELAY_SOURCE_RATE_PER_MINUTE",
        "RELAY_TRUST_PROXY_HEADERS",
        "RELAY_TRUSTED_PROXY_CIDRS",
        "RELAY_DEDUP_TTL_SECONDS",
        "RELAY_COOLDOWN_SECONDS",
        "RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW",
        "RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS",
        "RELAY_PUBLISH_QUEUE_CAPACITY",
        "RELAY_PUBLISH_MAX_RETRIES",
        "RELAY_PUBLISH_BACKOFF_BASE_MS",
        "RELAY_PUBLISH_BACKOFF_MAX_MS",
    ];

    struct EnvSnapshot {
        values: Vec<(String, Option<String>)>,
    }

    impl EnvSnapshot {
        fn capture(keys: &[&str]) -> Self {
            let values = keys
                .iter()
                .map(|key| ((*key).to_string(), env::var(key).ok()))
                .collect();
            Self { values }
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in self.values.drain(..) {
                // Safety: tests serialize all env access through ENV_LOCK.
                unsafe {
                    match value {
                        Some(value) => env::set_var(&key, value),
                        None => env::remove_var(&key),
                    }
                }
            }
        }
    }

    fn with_env(overrides: &[(&str, &str)], test_fn: impl FnOnce()) {
        let _lock = ENV_LOCK.lock().expect("lock env for test");
        let _snapshot = EnvSnapshot::capture(CONFIG_KEYS);

        for key in CONFIG_KEYS {
            // Safety: tests serialize all env access through ENV_LOCK.
            unsafe {
                env::remove_var(key);
            }
        }

        for (key, value) in overrides {
            // Safety: tests serialize all env access through ENV_LOCK.
            unsafe {
                env::set_var(key, value);
            }
        }

        test_fn();
    }

    fn base_required_env<'a>() -> [(&'a str, &'a str); 3] {
        [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
        ]
    }

    #[test]
    fn default_ssl_requires_tls_material() {
        let env_vars = base_required_env();
        with_env(&env_vars, || {
            let error = Config::from_env().expect_err("config should reject missing TLS vars");
            assert!(
                error
                    .to_string()
                    .contains("KAFKA_TLS_CERT is required when KAFKA_SECURITY_PROTOCOL=ssl")
            );
        });
    }

    #[test]
    fn ssl_mode_accepts_config_when_tls_material_is_present() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
            ("KAFKA_SECURITY_PROTOCOL", "ssl"),
            ("KAFKA_TLS_CERT", "/tmp/client.crt"),
            ("KAFKA_TLS_KEY", "/tmp/client.key"),
            ("KAFKA_TLS_CA", "/tmp/ca.crt"),
        ];
        with_env(&env_vars, || {
            let config = Config::from_env().expect("config should accept ssl with tls vars");
            assert_eq!(config.kafka_security_protocol, "ssl");
            assert!(!config.kafka_allow_plaintext);
        });
    }

    #[test]
    fn plaintext_requires_explicit_opt_in() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
            ("KAFKA_SECURITY_PROTOCOL", "plaintext"),
        ];
        with_env(&env_vars, || {
            let error = Config::from_env().expect_err("plaintext without opt-in must fail");
            assert!(
                error.to_string().contains(
                    "KAFKA_SECURITY_PROTOCOL=plaintext requires KAFKA_ALLOW_PLAINTEXT=true"
                )
            );
        });
    }

    #[test]
    fn plaintext_accepts_when_explicitly_allowed() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
            ("KAFKA_SECURITY_PROTOCOL", "plaintext"),
            ("KAFKA_ALLOW_PLAINTEXT", "true"),
        ];
        with_env(&env_vars, || {
            let config = Config::from_env().expect("plaintext should be allowed when opted in");
            assert_eq!(config.kafka_security_protocol, "plaintext");
            assert!(config.kafka_allow_plaintext);
        });
    }

    #[test]
    fn rejects_unknown_kafka_security_protocol() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
            ("KAFKA_SECURITY_PROTOCOL", "sasl_ssl"),
        ];
        with_env(&env_vars, || {
            let error = Config::from_env().expect_err("unknown protocol must be rejected");
            assert!(error.to_string().contains(
                "unsupported KAFKA_SECURITY_PROTOCOL=sasl_ssl; expected ssl or plaintext"
            ));
        });
    }
}
