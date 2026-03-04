use anyhow::{Context, Result, anyhow};
use ipnet::IpNet;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct ServeRouteRule {
    pub id: String,
    pub source_match: String,
    pub event_type_pattern: String,
    pub target_topic: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum RuntimeServePluginConfig {
    EventTypeAlias { from: String, to: String },
    RequirePayloadField { pointer: String },
    AddMetaFlag { flag: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum RuntimeIngressAdapter {
    HttpWebhookIngress {
        id: String,
        bind: String,
        path_template: String,
        #[serde(default)]
        plugins: Vec<RuntimeServePluginConfig>,
    },
    WebsocketIngress {
        id: String,
        path_template: String,
        auth_mode: String,
        #[serde(default)]
        token_env: Option<String>,
        #[serde(default)]
        plugins: Vec<RuntimeServePluginConfig>,
    },
    McpIngestExposed {
        id: String,
        tool_name: String,
        transport_driver: String,
        bind: String,
        auth_mode: String,
        #[serde(default)]
        token_env: Option<String>,
        max_payload_bytes: usize,
        path: String,
        #[serde(default)]
        plugins: Vec<RuntimeServePluginConfig>,
    },
    KafkaIngress {
        id: String,
        topics: Vec<String>,
        group_id: String,
        #[serde(default)]
        brokers: Option<String>,
        #[serde(default)]
        plugins: Vec<RuntimeServePluginConfig>,
    },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub enabled_sources: Vec<String>,
    pub source_topic_prefix: String,
    pub relay_source_topics: Vec<String>,
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
    pub hmac_secret_github: Option<String>,
    pub hmac_secret_linear: Option<String>,
    pub hmac_secret_example: Option<String>,
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
    pub validation_mode: String,
    pub active_profile: String,
    pub contract_path: Option<String>,
    pub active_ingress_adapter_id: Option<String>,
    pub ingress_adapters: Vec<RuntimeIngressAdapter>,
    pub serve_routes: Vec<ServeRouteRule>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let enabled_sources = env_csv_lower("RELAY_ENABLED_SOURCES", "github,linear")?;
        if enabled_sources.is_empty() {
            return Err(anyhow!("RELAY_ENABLED_SOURCES cannot be empty"));
        }

        let source_topic_prefix = env::var("RELAY_SOURCE_TOPIC_PREFIX")
            .unwrap_or_else(|_| "webhooks".to_string())
            .trim()
            .to_string();
        if source_topic_prefix.is_empty() {
            return Err(anyhow!(
                "RELAY_SOURCE_TOPIC_PREFIX cannot be empty when provided"
            ));
        }

        let relay_source_topics = match env::var("RELAY_SOURCE_TOPICS")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            Some(raw_topics) => {
                let parsed_topics = parse_csv(&raw_topics);
                if parsed_topics.is_empty() {
                    return Err(anyhow!("RELAY_SOURCE_TOPICS cannot be empty when provided"));
                }
                for source in &enabled_sources {
                    if !parsed_topics
                        .iter()
                        .any(|topic| topic_matches_source(topic, source))
                    {
                        return Err(anyhow!(
                            "RELAY_SOURCE_TOPICS must include a topic for enabled source {source}"
                        ));
                    }
                }
                parsed_topics
            }
            None => enabled_sources
                .iter()
                .map(|source| format!("{source_topic_prefix}.{source}"))
                .collect(),
        };

        let github_enabled = contains_source(&enabled_sources, "github");
        let linear_enabled = contains_source(&enabled_sources, "linear");
        let example_enabled = contains_source(&enabled_sources, "example");

        let config = Self {
            bind_addr: env::var("RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            enabled_sources,
            source_topic_prefix,
            relay_source_topics,
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
            hmac_secret_github: conditional_env("HMAC_SECRET_GITHUB", github_enabled)?,
            hmac_secret_linear: conditional_env("HMAC_SECRET_LINEAR", linear_enabled)?,
            hmac_secret_example: conditional_env("HMAC_SECRET_EXAMPLE", example_enabled)?,
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
            validation_mode: env::var("RELAY_VALIDATION_MODE")
                .unwrap_or_else(|_| "strict".to_string())
                .trim()
                .to_ascii_lowercase(),
            active_profile: env::var("RELAY_PROFILE")
                .unwrap_or_else(|_| "default-openclaw".to_string())
                .trim()
                .to_string(),
            contract_path: env::var("RELAY_CONTRACT_PATH")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            active_ingress_adapter_id: env::var("RELAY_INGRESS_ADAPTER_ID")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            ingress_adapters: parse_ingress_adapters_from_env()?,
            serve_routes: parse_serve_routes_from_env()?,
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

        match config.validation_mode.as_str() {
            "strict" | "debug" => {}
            other => {
                return Err(anyhow!(
                    "unsupported RELAY_VALIDATION_MODE={other}; expected strict or debug"
                ));
            }
        }

        for route in &config.serve_routes {
            if route.id.trim().is_empty() {
                return Err(anyhow!("RELAY_SERVE_ROUTES_JSON route id cannot be empty"));
            }
            if route.source_match.trim().is_empty() {
                return Err(anyhow!(
                    "RELAY_SERVE_ROUTES_JSON route source_match cannot be empty"
                ));
            }
            if route.event_type_pattern.trim().is_empty() {
                return Err(anyhow!(
                    "RELAY_SERVE_ROUTES_JSON route event_type_pattern cannot be empty"
                ));
            }
            if route.target_topic.trim().is_empty() {
                return Err(anyhow!(
                    "RELAY_SERVE_ROUTES_JSON route target_topic cannot be empty"
                ));
            }
        }

        for adapter in &config.ingress_adapters {
            match adapter {
                RuntimeIngressAdapter::HttpWebhookIngress {
                    id,
                    bind,
                    path_template,
                    plugins,
                } => {
                    if id.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON http_webhook_ingress id cannot be empty"
                        ));
                    }
                    if bind.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON http_webhook_ingress bind cannot be empty"
                        ));
                    }
                    if path_template.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON http_webhook_ingress path_template cannot be empty"
                        ));
                    }
                    validate_serve_plugins(plugins, id)?;
                }
                RuntimeIngressAdapter::WebsocketIngress {
                    id,
                    path_template,
                    auth_mode,
                    plugins,
                    ..
                } => {
                    if id.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON websocket_ingress id cannot be empty"
                        ));
                    }
                    if path_template.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON websocket_ingress path_template cannot be empty"
                        ));
                    }
                    if auth_mode.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON websocket_ingress auth_mode cannot be empty"
                        ));
                    }
                    validate_serve_plugins(plugins, id)?;
                }
                RuntimeIngressAdapter::McpIngestExposed {
                    id,
                    tool_name,
                    transport_driver,
                    bind,
                    auth_mode,
                    token_env: _,
                    max_payload_bytes,
                    path,
                    plugins,
                } => {
                    if id.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed id cannot be empty"
                        ));
                    }
                    if tool_name.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed tool_name cannot be empty"
                        ));
                    }
                    if transport_driver.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed transport_driver cannot be empty"
                        ));
                    }
                    if bind.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed bind cannot be empty"
                        ));
                    }
                    if auth_mode.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed auth_mode cannot be empty"
                        ));
                    }
                    if *max_payload_bytes == 0 {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed max_payload_bytes must be positive"
                        ));
                    }
                    if path.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON mcp_ingest_exposed path cannot be empty"
                        ));
                    }
                    validate_serve_plugins(plugins, id)?;
                }
                RuntimeIngressAdapter::KafkaIngress {
                    id,
                    topics,
                    group_id,
                    plugins,
                    ..
                } => {
                    if id.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON kafka_ingress id cannot be empty"
                        ));
                    }
                    if topics.is_empty() || topics.iter().any(|topic| topic.trim().is_empty()) {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON kafka_ingress topics must be non-empty"
                        ));
                    }
                    if group_id.trim().is_empty() {
                        return Err(anyhow!(
                            "RELAY_INGRESS_ADAPTERS_JSON kafka_ingress group_id cannot be empty"
                        ));
                    }
                    validate_serve_plugins(plugins, id)?;
                }
            }
        }

        Ok(config)
    }

    pub fn is_source_enabled(&self, source: &str) -> bool {
        let normalized = source.trim().to_ascii_lowercase();
        self.enabled_sources
            .iter()
            .any(|candidate| candidate == &normalized)
    }

    pub fn source_topic_name(&self, source: &str) -> String {
        let normalized_source = source.trim().to_ascii_lowercase();
        if let Some(topic) = self
            .relay_source_topics
            .iter()
            .find(|topic| topic_matches_source(topic, &normalized_source))
        {
            return topic.clone();
        }

        format!("{}.{}", self.source_topic_prefix, normalized_source)
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing required env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("required env var {name} cannot be empty"));
    }
    Ok(value)
}

fn conditional_env(name: &str, required: bool) -> Result<Option<String>> {
    if required {
        return required_env(name).map(Some);
    }

    Ok(env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn env_csv_lower(name: &str, default: &str) -> Result<Vec<String>> {
    let raw = env::var(name).unwrap_or_else(|_| default.to_string());
    let values = parse_csv(&raw)
        .into_iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(anyhow!("{name} cannot be empty"));
    }
    Ok(values)
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn contains_source(values: &[String], source: &str) -> bool {
    values.iter().any(|value| value == source)
}

fn topic_matches_source(topic: &str, source: &str) -> bool {
    let normalized_topic = topic.trim().to_ascii_lowercase();
    normalized_topic == source || normalized_topic.ends_with(&format!(".{source}"))
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

fn parse_serve_routes_from_env() -> Result<Vec<ServeRouteRule>> {
    let raw = match env::var("RELAY_SERVE_ROUTES_JSON") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str::<Vec<ServeRouteRule>>(trimmed)
        .with_context(|| "parse RELAY_SERVE_ROUTES_JSON as route list".to_string())
}

fn parse_ingress_adapters_from_env() -> Result<Vec<RuntimeIngressAdapter>> {
    let raw = match env::var("RELAY_INGRESS_ADAPTERS_JSON") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    serde_json::from_str::<Vec<RuntimeIngressAdapter>>(trimmed)
        .with_context(|| "parse RELAY_INGRESS_ADAPTERS_JSON as adapter list".to_string())
}

fn validate_serve_plugins(plugins: &[RuntimeServePluginConfig], adapter_id: &str) -> Result<()> {
    for plugin in plugins {
        match plugin {
            RuntimeServePluginConfig::EventTypeAlias { from, to } => {
                if from.trim().is_empty() || to.trim().is_empty() {
                    return Err(anyhow!(
                        "RELAY_INGRESS_ADAPTERS_JSON adapter '{}' event_type_alias plugin requires non-empty from/to",
                        adapter_id
                    ));
                }
            }
            RuntimeServePluginConfig::RequirePayloadField { pointer } => {
                if pointer.trim().is_empty() {
                    return Err(anyhow!(
                        "RELAY_INGRESS_ADAPTERS_JSON adapter '{}' require_payload_field plugin requires non-empty pointer",
                        adapter_id
                    ));
                }
                if !pointer.starts_with('/') {
                    return Err(anyhow!(
                        "RELAY_INGRESS_ADAPTERS_JSON adapter '{}' require_payload_field pointer must start with '/'",
                        adapter_id
                    ));
                }
            }
            RuntimeServePluginConfig::AddMetaFlag { flag } => {
                if flag.trim().is_empty() {
                    return Err(anyhow!(
                        "RELAY_INGRESS_ADAPTERS_JSON adapter '{}' add_meta_flag plugin requires non-empty flag",
                        adapter_id
                    ));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::env;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    const CONFIG_KEYS: &[&str] = &[
        "RELAY_BIND",
        "RELAY_ENABLED_SOURCES",
        "RELAY_SOURCE_TOPIC_PREFIX",
        "RELAY_SOURCE_TOPICS",
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
        "HMAC_SECRET_EXAMPLE",
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
        "RELAY_VALIDATION_MODE",
        "RELAY_PROFILE",
        "RELAY_CONTRACT_PATH",
        "RELAY_INGRESS_ADAPTER_ID",
        "RELAY_INGRESS_ADAPTERS_JSON",
        "RELAY_SERVE_ROUTES_JSON",
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
            assert_eq!(config.hmac_secret_github.as_deref(), Some("github-secret"));
            assert_eq!(config.hmac_secret_linear.as_deref(), Some("linear-secret"));
            assert_eq!(config.hmac_secret_example, None);
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

    #[test]
    fn allows_disabling_builtin_sources_without_their_secrets() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("RELAY_ENABLED_SOURCES", "github"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("KAFKA_SECURITY_PROTOCOL", "plaintext"),
            ("KAFKA_ALLOW_PLAINTEXT", "true"),
        ];
        with_env(&env_vars, || {
            let config = Config::from_env().expect("config should load for github-only mode");
            assert!(config.is_source_enabled("github"));
            assert!(!config.is_source_enabled("linear"));
            assert_eq!(config.hmac_secret_linear, None);
            assert_eq!(config.relay_source_topics, vec!["webhooks.github"]);
        });
    }

    #[test]
    fn accepts_explicit_source_topics_override() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("HMAC_SECRET_GITHUB", "github-secret"),
            ("HMAC_SECRET_LINEAR", "linear-secret"),
            ("RELAY_SOURCE_TOPICS", "custom.github,custom.linear"),
            ("KAFKA_SECURITY_PROTOCOL", "plaintext"),
            ("KAFKA_ALLOW_PLAINTEXT", "true"),
        ];
        with_env(&env_vars, || {
            let config = Config::from_env().expect("config should accept explicit source topics");
            assert_eq!(
                config.relay_source_topics,
                vec!["custom.github", "custom.linear"]
            );
            assert_eq!(config.source_topic_name("github"), "custom.github");
            assert_eq!(config.source_topic_name("linear"), "custom.linear");
        });
    }

    #[test]
    fn requires_example_secret_when_example_source_is_enabled() {
        let env_vars = [
            ("KAFKA_BROKERS", "broker:9093"),
            ("RELAY_ENABLED_SOURCES", "example"),
            ("KAFKA_SECURITY_PROTOCOL", "plaintext"),
            ("KAFKA_ALLOW_PLAINTEXT", "true"),
        ];
        with_env(&env_vars, || {
            let error = Config::from_env().expect_err("example source should require secret");
            assert!(
                error
                    .to_string()
                    .contains("missing required env var: HMAC_SECRET_EXAMPLE")
            );
        });
    }
}
