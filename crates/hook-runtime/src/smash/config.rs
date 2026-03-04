use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub kafka_brokers: String,
    pub kafka_sasl_username: Option<String>,
    pub kafka_sasl_password: Option<String>,
    pub kafka_security_protocol: String,
    pub kafka_sasl_mechanism: Option<String>,
    pub kafka_group_id: String,
    pub kafka_topics: Vec<String>,
    pub openclaw_message_max_bytes: usize,
    pub dlq_topic: String,
    pub backoff_base_seconds: u64,
    pub backoff_max_seconds: u64,
    pub smash_routes: Vec<SmashRouteConfig>,
    pub adapters: Vec<SmashAdapterConfig>,
    pub transports: Vec<SmashTransportConfig>,
    pub allow_no_output: bool,
    pub no_output_sink: Option<NoOutputSink>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmashRouteConfig {
    pub id: String,
    pub source_topic_pattern: String,
    #[serde(default)]
    pub event_filters: Vec<String>,
    #[serde(default)]
    pub destinations: Vec<RouteDestinationConfig>,
}

fn default_required_destination() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDestinationConfig {
    pub adapter_id: String,
    #[serde(default = "default_required_destination")]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum SmashAdapterConfig {
    OpenclawHttpOutput {
        id: String,
        url: String,
        token_env: String,
        timeout_seconds: u64,
        max_retries: u32,
        #[serde(default)]
        plugins: Vec<SmashPluginConfig>,
    },
    McpToolOutput {
        id: String,
        tool_name: String,
        transport_ref: String,
        #[serde(default)]
        plugins: Vec<SmashPluginConfig>,
    },
    WebsocketClientOutput {
        id: String,
        url: String,
        auth_mode: String,
        #[serde(default)]
        token_env: Option<String>,
        send_timeout_ms: u64,
        #[serde(default = "default_retry_max_retries")]
        retry_max_retries: u32,
        #[serde(default = "default_retry_backoff_ms")]
        retry_backoff_ms: u64,
        #[serde(default)]
        plugins: Vec<SmashPluginConfig>,
    },
    WebsocketServerOutput {
        id: String,
        bind: String,
        path: String,
        auth_mode: String,
        #[serde(default)]
        token_env: Option<String>,
        max_clients: usize,
        queue_depth_per_client: usize,
        send_timeout_ms: u64,
        #[serde(default)]
        plugins: Vec<SmashPluginConfig>,
    },
    KafkaOutput {
        id: String,
        topic: String,
        key_mode: String,
        #[serde(default)]
        plugins: Vec<SmashPluginConfig>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum SmashPluginConfig {
    EventTypeAlias { from: String, to: String },
    RequirePayloadField { pointer: String },
    AddMetaFlag { flag: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum SmashTransportConfig {
    StdioJsonrpc {
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: std::collections::BTreeMap<String, String>,
    },
    HttpSse {
        name: String,
        url: String,
        auth_mode: String,
        #[serde(default)]
        token_env: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoOutputSink {
    Discard,
    Dlq,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let kafka_topics_from_env = env::var("KAFKA_TOPICS")
            .ok()
            .map(|raw| parse_csv_topics(&raw))
            .filter(|topics| !topics.is_empty());
        let allow_no_output = env_bool("HOOK_ALLOW_NO_OUTPUT", false);
        let no_output_sink = parse_no_output_sink(env::var("HOOK_NO_OUTPUT_SINK").ok())?;

        let smash_routes_from_env = parse_routes_json_env()?;
        let adapters_from_env = parse_adapters_json_env()?;
        let transports_from_env = parse_transports_json_env()?;

        let (smash_routes, adapters, transports, using_legacy_fallback) = if !smash_routes_from_env
            .is_empty()
            || !adapters_from_env.is_empty()
        {
            if smash_routes_from_env.is_empty() {
                return Err(anyhow!(
                    "HOOK_SMASH_ROUTES_JSON is required when HOOK_SMASH_ADAPTERS_JSON is provided"
                ));
            }
            if adapters_from_env.is_empty() {
                return Err(anyhow!(
                    "HOOK_SMASH_ADAPTERS_JSON is required when HOOK_SMASH_ROUTES_JSON is provided"
                ));
            }
            (
                smash_routes_from_env,
                adapters_from_env,
                transports_from_env,
                false,
            )
        } else {
            let default_adapter_id = "openclaw-output".to_string();
            let adapter = SmashAdapterConfig::OpenclawHttpOutput {
                id: default_adapter_id.clone(),
                url: required_env("OPENCLAW_WEBHOOK_URL")?,
                token_env: "OPENCLAW_WEBHOOK_TOKEN".to_string(),
                timeout_seconds: env_u64("OPENCLAW_HTTP_TIMEOUT_SECONDS", 20)?,
                max_retries: env_u32("CONSUMER_MAX_RETRIES", 5)?,
                plugins: Vec::new(),
            };
            let fallback_topics = kafka_topics_from_env.clone().unwrap_or_else(|| {
                vec!["webhooks.github".to_string(), "webhooks.linear".to_string()]
            });
            let routes = fallback_topics
                .iter()
                .map(|topic| SmashRouteConfig {
                    id: format!("legacy-{}", topic.replace('.', "-")),
                    source_topic_pattern: topic.clone(),
                    event_filters: Vec::new(),
                    destinations: vec![RouteDestinationConfig {
                        adapter_id: default_adapter_id.clone(),
                        required: true,
                    }],
                })
                .collect::<Vec<_>>();
            (routes, vec![adapter], Vec::new(), true)
        };

        let kafka_topics = match kafka_topics_from_env {
            Some(topics) => topics,
            None => derive_topics_from_routes(&smash_routes)?,
        };
        if kafka_topics.is_empty() {
            return Err(anyhow!("KAFKA_TOPICS cannot be empty"));
        }

        let config = Self {
            kafka_brokers: required_env("KAFKA_BROKERS")?,
            kafka_sasl_username: env::var("KAFKA_SASL_USERNAME")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            kafka_sasl_password: env::var("KAFKA_SASL_PASSWORD")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            kafka_security_protocol: env::var("KAFKA_SECURITY_PROTOCOL")
                .unwrap_or_else(|_| "PLAINTEXT".to_string()),
            kafka_sasl_mechanism: env::var("KAFKA_SASL_MECHANISM")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            kafka_group_id: env::var("KAFKA_GROUP_ID")
                .unwrap_or_else(|_| "kafka-openclaw-hook".to_string()),
            kafka_topics,
            openclaw_message_max_bytes: env_usize("OPENCLAW_MESSAGE_MAX_BYTES", 4_000)?,
            dlq_topic: env::var("KAFKA_DLQ_TOPIC").unwrap_or_else(|_| "webhooks.dlq".to_string()),
            backoff_base_seconds: env_u64("CONSUMER_BACKOFF_BASE_SECONDS", 1)?,
            backoff_max_seconds: env_u64("CONSUMER_BACKOFF_MAX_SECONDS", 30)?,
            smash_routes,
            adapters,
            transports,
            allow_no_output,
            no_output_sink,
        };

        config.validate(using_legacy_fallback)?;
        Ok(config)
    }

    fn validate(&self, using_legacy_fallback: bool) -> Result<()> {
        if self.openclaw_message_max_bytes < 128 {
            return Err(anyhow!("OPENCLAW_MESSAGE_MAX_BYTES must be at least 128"));
        }

        if self.dlq_topic.trim().is_empty() {
            return Err(anyhow!("KAFKA_DLQ_TOPIC cannot be empty"));
        }

        let mut adapter_ids = BTreeSet::new();
        for adapter in &self.adapters {
            let adapter_id = adapter_id(adapter);
            if adapter_id.trim().is_empty() {
                return Err(anyhow!("smash adapter id cannot be empty"));
            }
            if !adapter_ids.insert(adapter_id.to_string()) {
                return Err(anyhow!("duplicate smash adapter id '{}'", adapter_id));
            }
            match adapter {
                SmashAdapterConfig::OpenclawHttpOutput {
                    url,
                    token_env,
                    timeout_seconds,
                    plugins,
                    ..
                } => {
                    if url.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' url cannot be empty",
                            adapter_id
                        ));
                    }
                    if token_env.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' token_env cannot be empty",
                            adapter_id
                        ));
                    }
                    if *timeout_seconds == 0 {
                        return Err(anyhow!(
                            "smash adapter '{}' timeout_seconds must be greater than 0",
                            adapter_id
                        ));
                    }
                    validate_smash_plugins(plugins, adapter_id)?;
                }
                SmashAdapterConfig::McpToolOutput {
                    tool_name,
                    transport_ref,
                    plugins,
                    ..
                } => {
                    if tool_name.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' tool_name cannot be empty",
                            adapter_id
                        ));
                    }
                    if transport_ref.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' transport_ref cannot be empty",
                            adapter_id
                        ));
                    }
                    if !self
                        .transports
                        .iter()
                        .any(|transport| transport_name(transport) == transport_ref)
                    {
                        return Err(anyhow!(
                            "smash adapter '{}' references unknown transport '{}'",
                            adapter_id,
                            transport_ref
                        ));
                    }
                    validate_smash_plugins(plugins, adapter_id)?;
                }
                SmashAdapterConfig::WebsocketClientOutput {
                    url,
                    auth_mode,
                    token_env,
                    send_timeout_ms,
                    plugins,
                    ..
                } => {
                    if url.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' url cannot be empty",
                            adapter_id
                        ));
                    }
                    if auth_mode.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' auth_mode cannot be empty",
                            adapter_id
                        ));
                    }
                    if *send_timeout_ms == 0 {
                        return Err(anyhow!(
                            "smash adapter '{}' send_timeout_ms must be greater than 0",
                            adapter_id
                        ));
                    }
                    if !matches!(auth_mode.trim(), "none" | "bearer" | "hmac") {
                        return Err(anyhow!(
                            "smash adapter '{}' unsupported auth_mode '{}'",
                            adapter_id,
                            auth_mode
                        ));
                    }
                    if auth_mode.trim() != "none"
                        && token_env
                            .as_ref()
                            .map(|token| token.trim().is_empty())
                            .unwrap_or(true)
                    {
                        return Err(anyhow!(
                            "smash adapter '{}' auth_mode '{}' requires token_env",
                            adapter_id,
                            auth_mode
                        ));
                    }
                    validate_smash_plugins(plugins, adapter_id)?;
                }
                SmashAdapterConfig::WebsocketServerOutput {
                    bind,
                    path,
                    auth_mode,
                    token_env,
                    max_clients,
                    queue_depth_per_client,
                    send_timeout_ms,
                    plugins,
                    ..
                } => {
                    if bind.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' bind cannot be empty",
                            adapter_id
                        ));
                    }
                    if path.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' path cannot be empty",
                            adapter_id
                        ));
                    }
                    if auth_mode.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' auth_mode cannot be empty",
                            adapter_id
                        ));
                    }
                    if *max_clients == 0 {
                        return Err(anyhow!(
                            "smash adapter '{}' max_clients must be positive",
                            adapter_id
                        ));
                    }
                    if *queue_depth_per_client == 0 {
                        return Err(anyhow!(
                            "smash adapter '{}' queue_depth_per_client must be positive",
                            adapter_id
                        ));
                    }
                    if *send_timeout_ms == 0 {
                        return Err(anyhow!(
                            "smash adapter '{}' send_timeout_ms must be greater than 0",
                            adapter_id
                        ));
                    }
                    if auth_mode.trim() != "none"
                        && token_env
                            .as_ref()
                            .map(|token| token.trim().is_empty())
                            .unwrap_or(true)
                    {
                        return Err(anyhow!(
                            "smash adapter '{}' auth_mode '{}' requires token_env",
                            adapter_id,
                            auth_mode
                        ));
                    }
                    validate_smash_plugins(plugins, adapter_id)?;
                }
                SmashAdapterConfig::KafkaOutput {
                    topic,
                    key_mode,
                    plugins,
                    ..
                } => {
                    if topic.trim().is_empty() {
                        return Err(anyhow!(
                            "smash adapter '{}' topic cannot be empty",
                            adapter_id
                        ));
                    }
                    if !matches!(key_mode.trim(), "event_id" | "source" | "none") {
                        return Err(anyhow!(
                            "smash adapter '{}' key_mode must be event_id|source|none",
                            adapter_id
                        ));
                    }
                    validate_smash_plugins(plugins, adapter_id)?;
                }
            }
        }

        let mut route_ids = BTreeSet::new();
        let mut active_destinations = 0usize;
        for route in &self.smash_routes {
            if route.id.trim().is_empty() {
                return Err(anyhow!("smash route id cannot be empty"));
            }
            if !route_ids.insert(route.id.clone()) {
                return Err(anyhow!("duplicate smash route id '{}'", route.id));
            }
            if route.source_topic_pattern.trim().is_empty() {
                return Err(anyhow!(
                    "smash route '{}' source_topic_pattern cannot be empty",
                    route.id
                ));
            }
            for destination in &route.destinations {
                if !adapter_ids.contains(&destination.adapter_id) {
                    return Err(anyhow!(
                        "smash route '{}' references unknown adapter '{}'",
                        route.id,
                        destination.adapter_id
                    ));
                }
                active_destinations = active_destinations.saturating_add(1);
            }
        }

        if active_destinations == 0 {
            if !self.allow_no_output {
                return Err(anyhow!(
                    "profile has zero active smash outputs and HOOK_ALLOW_NO_OUTPUT is false"
                ));
            }
            if self.no_output_sink.is_none() {
                return Err(anyhow!(
                    "HOOK_ALLOW_NO_OUTPUT=true requires HOOK_NO_OUTPUT_SINK=discard|dlq"
                ));
            }
            if using_legacy_fallback {
                return Err(anyhow!(
                    "legacy smash mode cannot run with zero outputs; provide OPENCLAW_WEBHOOK_URL/OPENCLAW_WEBHOOK_TOKEN"
                ));
            }
        }

        Ok(())
    }
}

fn default_retry_max_retries() -> u32 {
    5
}

fn default_retry_backoff_ms() -> u64 {
    500
}

impl SmashAdapterConfig {
    pub fn id(&self) -> &str {
        adapter_id(self)
    }

    pub fn plugins(&self) -> &[SmashPluginConfig] {
        match self {
            SmashAdapterConfig::OpenclawHttpOutput { plugins, .. }
            | SmashAdapterConfig::McpToolOutput { plugins, .. }
            | SmashAdapterConfig::WebsocketClientOutput { plugins, .. }
            | SmashAdapterConfig::WebsocketServerOutput { plugins, .. }
            | SmashAdapterConfig::KafkaOutput { plugins, .. } => plugins.as_slice(),
        }
    }
}

fn adapter_id(adapter: &SmashAdapterConfig) -> &str {
    match adapter {
        SmashAdapterConfig::OpenclawHttpOutput { id, .. }
        | SmashAdapterConfig::McpToolOutput { id, .. }
        | SmashAdapterConfig::WebsocketClientOutput { id, .. }
        | SmashAdapterConfig::WebsocketServerOutput { id, .. }
        | SmashAdapterConfig::KafkaOutput { id, .. } => id.as_str(),
    }
}

fn transport_name(transport: &SmashTransportConfig) -> &str {
    match transport {
        SmashTransportConfig::StdioJsonrpc { name, .. }
        | SmashTransportConfig::HttpSse { name, .. } => name.as_str(),
    }
}

fn validate_smash_plugins(plugins: &[SmashPluginConfig], adapter_id: &str) -> Result<()> {
    for plugin in plugins {
        match plugin {
            SmashPluginConfig::EventTypeAlias { from, to } => {
                if from.trim().is_empty() || to.trim().is_empty() {
                    return Err(anyhow!(
                        "smash adapter '{}' event_type_alias plugin requires non-empty from/to",
                        adapter_id
                    ));
                }
            }
            SmashPluginConfig::RequirePayloadField { pointer } => {
                if pointer.trim().is_empty() {
                    return Err(anyhow!(
                        "smash adapter '{}' require_payload_field plugin requires non-empty pointer",
                        adapter_id
                    ));
                }
                if !pointer.starts_with('/') {
                    return Err(anyhow!(
                        "smash adapter '{}' require_payload_field pointer must start with '/'",
                        adapter_id
                    ));
                }
            }
            SmashPluginConfig::AddMetaFlag { flag } => {
                if flag.trim().is_empty() {
                    return Err(anyhow!(
                        "smash adapter '{}' add_meta_flag plugin requires non-empty flag",
                        adapter_id
                    ));
                }
            }
        }
    }

    Ok(())
}

fn parse_routes_json_env() -> Result<Vec<SmashRouteConfig>> {
    let raw = match env::var("HOOK_SMASH_ROUTES_JSON") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<SmashRouteConfig>>(&raw)
        .with_context(|| "parse HOOK_SMASH_ROUTES_JSON".to_string())
}

fn parse_adapters_json_env() -> Result<Vec<SmashAdapterConfig>> {
    let raw = match env::var("HOOK_SMASH_ADAPTERS_JSON") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<SmashAdapterConfig>>(&raw)
        .with_context(|| "parse HOOK_SMASH_ADAPTERS_JSON".to_string())
}

fn parse_transports_json_env() -> Result<Vec<SmashTransportConfig>> {
    let raw = match env::var("HOOK_SMASH_TRANSPORTS_JSON") {
        Ok(value) => value,
        Err(_) => return Ok(Vec::new()),
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<SmashTransportConfig>>(&raw)
        .with_context(|| "parse HOOK_SMASH_TRANSPORTS_JSON".to_string())
}

fn derive_topics_from_routes(routes: &[SmashRouteConfig]) -> Result<Vec<String>> {
    let mut topics = BTreeSet::new();
    for route in routes {
        let pattern = route.source_topic_pattern.trim();
        if pattern.is_empty() {
            continue;
        }
        if pattern.contains('*') {
            return Err(anyhow!(
                "route '{}' has wildcard source_topic_pattern '{}'; set explicit KAFKA_TOPICS",
                route.id,
                pattern
            ));
        }
        topics.insert(pattern.to_string());
    }
    Ok(topics.into_iter().collect())
}

fn parse_no_output_sink(raw: Option<String>) -> Result<Option<NoOutputSink>> {
    match raw {
        None => Ok(None),
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "" => Ok(None),
            "discard" => Ok(Some(NoOutputSink::Discard)),
            "dlq" => Ok(Some(NoOutputSink::Dlq)),
            other => Err(anyhow!(
                "invalid HOOK_NO_OUTPUT_SINK='{}'; expected discard or dlq",
                other
            )),
        },
    }
}

fn parse_csv_topics(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .map(ToString::to_string)
        .collect()
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
