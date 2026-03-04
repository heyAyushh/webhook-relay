use crate::capabilities::resolve_smash_backend;
use crate::cli::SmashArgs;
use crate::config::AppContext;
use anyhow::{Result, anyhow};
use relay_core::contract::{AppContract, EgressDriver, TransportDriver, ValidationMode};
use relay_core::contract_validator::validate_contract;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use toml::Value;

use super::serve::run_shell_backend;

pub async fn run(context: &AppContext, arguments: &SmashArgs) -> Result<()> {
    let backend = resolve_smash_backend(context);
    let mut contract_overrides = Vec::new();
    let mut requires_legacy_openclaw_env = true;

    if let Some(contract) = context.contract.as_ref() {
        let active = resolve_active_smash_contract(context, contract)?;
        if !active.adapters.is_empty() || active.allow_no_output {
            contract_overrides.push((
                "HOOK_SMASH_ADAPTERS_JSON".to_string(),
                serde_json::to_string(&active.adapters)?,
            ));
            contract_overrides.push((
                "HOOK_SMASH_ROUTES_JSON".to_string(),
                serde_json::to_string(&active.routes)?,
            ));
            contract_overrides.push((
                "HOOK_SMASH_TRANSPORTS_JSON".to_string(),
                serde_json::to_string(&active.transports)?,
            ));
            contract_overrides.push((
                "HOOK_ALLOW_NO_OUTPUT".to_string(),
                active.allow_no_output.to_string(),
            ));
            if let Some(no_output_sink) = active.no_output_sink {
                contract_overrides.push(("HOOK_NO_OUTPUT_SINK".to_string(), no_output_sink));
            }
            if !active.kafka_topics.is_empty() {
                contract_overrides.push((
                    "KAFKA_TOPICS".to_string(),
                    active
                        .kafka_topics
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(","),
                ));
            }
            requires_legacy_openclaw_env = false;
        }
    }

    let mut reasons = Vec::new();
    if backend.is_none() {
        reasons.push("no smash backend found".to_string());
    }

    if value(context, arguments.brokers.as_deref(), "KAFKA_BROKERS").is_none() {
        reasons.push("missing KAFKA_BROKERS".to_string());
    }
    if requires_legacy_openclaw_env {
        if value(
            context,
            arguments.webhook_url.as_deref(),
            "OPENCLAW_WEBHOOK_URL",
        )
        .is_none()
        {
            reasons.push("missing OPENCLAW_WEBHOOK_URL".to_string());
        }
        if value(
            context,
            arguments.webhook_token.as_deref(),
            "OPENCLAW_WEBHOOK_TOKEN",
        )
        .is_none()
        {
            reasons.push("missing OPENCLAW_WEBHOOK_TOKEN".to_string());
        }
    }

    if !reasons.is_empty() && !context.global.force {
        return Err(anyhow!(
            "smash unavailable: {}. use --force to bypass",
            reasons.join("; ")
        ));
    }

    let backend_spec = backend.ok_or_else(|| anyhow!("no smash backend resolved"))?;

    let mut overrides = contract_overrides;
    if let Some(brokers) = &arguments.brokers {
        overrides.push(("KAFKA_BROKERS".to_string(), brokers.clone()));
    }
    if let Some(topics) = &arguments.topics {
        overrides.push(("KAFKA_TOPICS".to_string(), topics.clone()));
    }
    if let Some(group_id) = &arguments.group_id {
        overrides.push(("KAFKA_GROUP_ID".to_string(), group_id.clone()));
    }
    if let Some(webhook_url) = &arguments.webhook_url {
        overrides.push(("OPENCLAW_WEBHOOK_URL".to_string(), webhook_url.clone()));
    }
    if let Some(webhook_token) = &arguments.webhook_token {
        overrides.push(("OPENCLAW_WEBHOOK_TOKEN".to_string(), webhook_token.clone()));
    }
    if let Some(instance_id) = &arguments.instance_id {
        overrides.push(("HOOK_INSTANCE_ID".to_string(), instance_id.clone()));
        overrides.push(("KAFKA_CLIENT_ID".to_string(), instance_id.clone()));
    }

    run_shell_backend(context, &backend_spec, &overrides)
}

#[derive(Debug, Clone)]
struct ActiveSmashContract {
    adapters: Vec<SmashAdapterEnv>,
    routes: Vec<SmashRouteEnv>,
    transports: Vec<SmashTransportEnv>,
    kafka_topics: BTreeSet<String>,
    allow_no_output: bool,
    no_output_sink: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
enum SmashAdapterEnv {
    OpenclawHttpOutput {
        id: String,
        url: String,
        token_env: String,
        timeout_seconds: u64,
        max_retries: u32,
        plugins: Vec<SmashPluginEnv>,
    },
    McpToolOutput {
        id: String,
        tool_name: String,
        transport_ref: String,
        plugins: Vec<SmashPluginEnv>,
    },
    WebsocketClientOutput {
        id: String,
        url: String,
        auth_mode: String,
        token_env: Option<String>,
        send_timeout_ms: u64,
        retry_max_retries: u32,
        retry_backoff_ms: u64,
        plugins: Vec<SmashPluginEnv>,
    },
    WebsocketServerOutput {
        id: String,
        bind: String,
        path: String,
        auth_mode: String,
        token_env: Option<String>,
        max_clients: usize,
        queue_depth_per_client: usize,
        send_timeout_ms: u64,
        plugins: Vec<SmashPluginEnv>,
    },
    KafkaOutput {
        id: String,
        topic: String,
        key_mode: String,
        plugins: Vec<SmashPluginEnv>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
enum SmashPluginEnv {
    EventTypeAlias { from: String, to: String },
    RequirePayloadField { pointer: String },
    AddMetaFlag { flag: String },
}

#[derive(Debug, Clone, Serialize)]
struct SmashRouteEnv {
    id: String,
    source_topic_pattern: String,
    event_filters: Vec<String>,
    destinations: Vec<SmashDestinationEnv>,
}

#[derive(Debug, Clone, Serialize)]
struct SmashDestinationEnv {
    adapter_id: String,
    required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
enum SmashTransportEnv {
    StdioJsonrpc {
        name: String,
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    HttpSse {
        name: String,
        url: String,
        auth_mode: String,
        token_env: Option<String>,
    },
}

fn resolve_active_smash_contract(
    context: &AppContext,
    contract: &AppContract,
) -> Result<ActiveSmashContract> {
    let mut contract_for_validation = contract.clone();
    if let Some(validation_mode) = context.global.validation_mode.as_deref() {
        contract_for_validation.policies.validation_mode = parse_validation_mode(validation_mode)?;
    }

    let validation = validate_contract(&contract_for_validation, &context.global.profile);
    let validated_profile = match validation {
        Ok(validated) => validated,
        Err(errors) => {
            let has_security_critical = errors.iter().any(|error| error.security_critical);
            if has_security_critical || !context.global.force {
                return Err(anyhow!(
                    "smash contract validation failed:\n{}",
                    errors
                        .iter()
                        .map(|error| format!(
                            "- [{}] {}{}",
                            error.code,
                            error.message,
                            if error.security_critical {
                                " (security-critical)"
                            } else {
                                ""
                            }
                        ))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            return Ok(ActiveSmashContract {
                adapters: Vec::new(),
                routes: Vec::new(),
                transports: Vec::new(),
                kafka_topics: BTreeSet::new(),
                allow_no_output: contract_for_validation.policies.allow_no_output,
                no_output_sink: contract_for_validation
                    .policies
                    .no_output_sink
                    .map(no_output_sink_to_string),
            });
        }
    };

    let adapters = contract
        .smash
        .egress_adapters
        .iter()
        .filter(|adapter| {
            validated_profile
                .smash_adapter_ids
                .iter()
                .any(|active_adapter_id| active_adapter_id == &adapter.id)
        })
        .map(to_smash_adapter_env)
        .collect::<Result<Vec<_>>>()?;

    let mut kafka_topics = BTreeSet::new();
    let routes = contract
        .smash
        .routes
        .iter()
        .filter(|route| {
            validated_profile
                .smash_route_ids
                .iter()
                .any(|active_route_id| active_route_id == &route.id)
        })
        .map(|route| {
            let source_topic_pattern = route.source_topic_pattern.trim().to_string();
            if source_topic_pattern.is_empty() {
                return Err(anyhow!(
                    "smash route '{}' has empty source_topic_pattern",
                    route.id
                ));
            }
            if source_topic_pattern.contains('*') {
                return Err(anyhow!(
                    "smash route '{}' uses wildcard source_topic_pattern '{}'; pass explicit --topics or use literal topics",
                    route.id,
                    source_topic_pattern
                ));
            }
            kafka_topics.insert(source_topic_pattern.clone());

            Ok(SmashRouteEnv {
                id: route.id.clone(),
                source_topic_pattern,
                event_filters: route.event_filters.clone(),
                destinations: route
                    .destinations
                    .iter()
                    .map(|destination| SmashDestinationEnv {
                        adapter_id: destination.adapter_id.clone(),
                        required: destination.required,
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut transports = Vec::new();
    for adapter in &adapters {
        if let SmashAdapterEnv::McpToolOutput { transport_ref, .. } = adapter {
            let Some(transport) = contract.transports.get(transport_ref) else {
                return Err(anyhow!(
                    "missing transport '{}' for mcp_tool_output",
                    transport_ref
                ));
            };
            transports.push(to_smash_transport_env(transport_ref, transport)?);
        }
    }

    Ok(ActiveSmashContract {
        adapters,
        routes,
        transports,
        kafka_topics,
        allow_no_output: contract_for_validation.policies.allow_no_output,
        no_output_sink: contract_for_validation
            .policies
            .no_output_sink
            .map(no_output_sink_to_string),
    })
}

fn to_smash_adapter_env(adapter: &relay_core::contract::EgressAdapter) -> Result<SmashAdapterEnv> {
    let plugins = parse_plugins_from_config(&adapter.config, &adapter.id)?;
    match adapter.driver {
        EgressDriver::OpenclawHttpOutput => Ok(SmashAdapterEnv::OpenclawHttpOutput {
            id: adapter.id.clone(),
            url: required_string_config(&adapter.config, "url", &adapter.id)?,
            token_env: required_string_config(&adapter.config, "token_env", &adapter.id)?,
            timeout_seconds: required_u64_config(&adapter.config, "timeout_seconds", &adapter.id)?,
            max_retries: required_u32_config(&adapter.config, "max_retries", &adapter.id)?,
            plugins,
        }),
        EgressDriver::McpToolOutput => Ok(SmashAdapterEnv::McpToolOutput {
            id: adapter.id.clone(),
            tool_name: required_string_config(&adapter.config, "tool_name", &adapter.id)?,
            transport_ref: required_string_config(&adapter.config, "transport_ref", &adapter.id)?,
            plugins,
        }),
        EgressDriver::WebsocketClientOutput => {
            let (retry_max_retries, retry_backoff_ms) =
                parse_retry_policy(&adapter.config, &adapter.id)?;
            Ok(SmashAdapterEnv::WebsocketClientOutput {
                id: adapter.id.clone(),
                url: required_string_config(&adapter.config, "url", &adapter.id)?,
                auth_mode: required_string_config(&adapter.config, "auth_mode", &adapter.id)?,
                token_env: optional_string_config(&adapter.config, "token_env"),
                send_timeout_ms: required_u64_config(
                    &adapter.config,
                    "send_timeout_ms",
                    &adapter.id,
                )?,
                retry_max_retries,
                retry_backoff_ms,
                plugins,
            })
        }
        EgressDriver::WebsocketServerOutput => Ok(SmashAdapterEnv::WebsocketServerOutput {
            id: adapter.id.clone(),
            bind: required_string_config(&adapter.config, "bind", &adapter.id)?,
            path: required_string_config(&adapter.config, "path", &adapter.id)?,
            auth_mode: required_string_config(&adapter.config, "auth_mode", &adapter.id)?,
            token_env: optional_string_config(&adapter.config, "token_env"),
            max_clients: required_usize_config(&adapter.config, "max_clients", &adapter.id)?,
            queue_depth_per_client: required_usize_config(
                &adapter.config,
                "queue_depth_per_client",
                &adapter.id,
            )?,
            send_timeout_ms: required_u64_config(&adapter.config, "send_timeout_ms", &adapter.id)?,
            plugins,
        }),
        EgressDriver::KafkaOutput => Ok(SmashAdapterEnv::KafkaOutput {
            id: adapter.id.clone(),
            topic: required_string_config(&adapter.config, "topic", &adapter.id)?,
            key_mode: required_string_config(&adapter.config, "key_mode", &adapter.id)?,
            plugins,
        }),
        EgressDriver::Unknown(_) => Err(anyhow!(
            "active smash adapter '{}' uses unsupported driver '{}'",
            adapter.id,
            adapter.driver.as_str()
        )),
    }
}

fn parse_plugins_from_config(
    config: &BTreeMap<String, Value>,
    adapter_id: &str,
) -> Result<Vec<SmashPluginEnv>> {
    let Some(value) = config.get("plugins") else {
        return Ok(Vec::new());
    };

    let Value::Array(items) = value else {
        return Err(anyhow!(
            "adapter '{}' key 'plugins' must be an array",
            adapter_id
        ));
    };

    let mut plugins = Vec::with_capacity(items.len());
    for item in items {
        let plugin = item.clone().try_into::<SmashPluginEnv>().map_err(|error| {
            anyhow!(
                "adapter '{}' has invalid plugin config: {}",
                adapter_id,
                error
            )
        })?;
        plugins.push(plugin);
    }
    Ok(plugins)
}

fn to_smash_transport_env(
    name: &str,
    transport: &relay_core::contract::TransportDef,
) -> Result<SmashTransportEnv> {
    match transport.driver {
        TransportDriver::StdioJsonrpc => {
            let command = required_string_config(&transport.config, "command", name)?;
            let args = optional_string_array_config(&transport.config, "args");
            let env = optional_string_map_config(&transport.config, "env", name)?;
            Ok(SmashTransportEnv::StdioJsonrpc {
                name: name.to_string(),
                command,
                args,
                env,
            })
        }
        TransportDriver::HttpSse => Ok(SmashTransportEnv::HttpSse {
            name: name.to_string(),
            url: required_string_config(&transport.config, "url", name)?,
            auth_mode: required_string_config(&transport.config, "auth_mode", name)?,
            token_env: optional_string_config(&transport.config, "token_env"),
        }),
        TransportDriver::Unknown(_) => Err(anyhow!(
            "active transport '{}' uses unsupported driver '{}'",
            name,
            transport.driver.as_str()
        )),
    }
}

fn parse_retry_policy(config: &BTreeMap<String, Value>, adapter_id: &str) -> Result<(u32, u64)> {
    match config.get("retry_policy") {
        None => Ok((5, 500)),
        Some(Value::Table(table)) => {
            let max_retries = match table.get("max_retries") {
                Some(Value::Integer(value)) if *value >= 0 => {
                    u32::try_from(*value).map_err(|_| {
                        anyhow!(
                            "smash adapter '{}' retry_policy.max_retries exceeds u32 range",
                            adapter_id
                        )
                    })?
                }
                Some(_) => {
                    return Err(anyhow!(
                        "smash adapter '{}' retry_policy.max_retries must be integer",
                        adapter_id
                    ));
                }
                None => 5,
            };
            let backoff_ms = match table.get("backoff_ms") {
                Some(Value::Integer(value)) if *value >= 0 => *value as u64,
                Some(_) => {
                    return Err(anyhow!(
                        "smash adapter '{}' retry_policy.backoff_ms must be integer",
                        adapter_id
                    ));
                }
                None => 500,
            };
            Ok((max_retries, backoff_ms))
        }
        Some(_) => Err(anyhow!(
            "smash adapter '{}' retry_policy must be a table",
            adapter_id
        )),
    }
}

fn value(context: &AppContext, flag: Option<&str>, key: &str) -> Option<String> {
    context.resolve_value(flag, key)
}

fn parse_validation_mode(raw: &str) -> Result<ValidationMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "strict" => Ok(ValidationMode::Strict),
        "debug" => Ok(ValidationMode::Debug),
        other => Err(anyhow!(
            "invalid --validation-mode '{}'; expected strict or debug",
            other
        )),
    }
}

fn required_string_config(
    config: &BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<String> {
    match config.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Some(_) => Err(anyhow!(
            "adapter '{}' key '{}' must be a non-empty string",
            adapter_id,
            key
        )),
        None => Err(anyhow!(
            "adapter '{}' missing required key '{}'",
            adapter_id,
            key
        )),
    }
}

fn optional_string_config(config: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    match config.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn required_u64_config(
    config: &BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<u64> {
    match config.get(key) {
        Some(Value::Integer(value)) if *value >= 0 => Ok(*value as u64),
        Some(_) => Err(anyhow!(
            "adapter '{}' key '{}' must be a non-negative integer",
            adapter_id,
            key
        )),
        None => Err(anyhow!(
            "adapter '{}' missing required key '{}'",
            adapter_id,
            key
        )),
    }
}

fn required_u32_config(
    config: &BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<u32> {
    let value = required_u64_config(config, key, adapter_id)?;
    u32::try_from(value)
        .map_err(|_| anyhow!("adapter '{}' key '{}' exceeds u32 range", adapter_id, key))
}

fn required_usize_config(
    config: &BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<usize> {
    let value = required_u64_config(config, key, adapter_id)?;
    usize::try_from(value)
        .map_err(|_| anyhow!("adapter '{}' key '{}' exceeds usize range", adapter_id, key))
}

fn optional_string_array_config(config: &BTreeMap<String, Value>, key: &str) -> Vec<String> {
    match config.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    }
}

fn optional_string_map_config(
    config: &BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<BTreeMap<String, String>> {
    match config.get(key) {
        None => Ok(BTreeMap::new()),
        Some(Value::Table(table)) => {
            let mut values = BTreeMap::new();
            for (entry_key, entry_value) in table {
                match entry_value {
                    Value::String(value) => {
                        values.insert(entry_key.clone(), value.clone());
                    }
                    _ => {
                        return Err(anyhow!(
                            "adapter '{}' key '{}.{}' must be string",
                            adapter_id,
                            key,
                            entry_key
                        ));
                    }
                }
            }
            Ok(values)
        }
        Some(_) => Err(anyhow!(
            "adapter '{}' key '{}' must be table",
            adapter_id,
            key
        )),
    }
}

fn no_output_sink_to_string(sink: relay_core::contract::NoOutputSink) -> String {
    match sink {
        relay_core::contract::NoOutputSink::Discard => "discard",
        relay_core::contract::NoOutputSink::Dlq => "dlq",
    }
    .to_string()
}
