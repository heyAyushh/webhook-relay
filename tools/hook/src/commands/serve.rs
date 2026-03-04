use crate::capabilities::resolve_serve_backend;
use crate::cli::ServeArgs;
use crate::config::AppContext;
use anyhow::{Result, anyhow};
use relay_core::contract::{AppContract, IngressDriver, ValidationMode};
use relay_core::contract_validator::validate_contract;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use toml::Value;

pub async fn run(context: &AppContext, arguments: &ServeArgs) -> Result<()> {
    let backend = resolve_serve_backend(context);

    let mut reasons = Vec::new();
    if backend.is_none() {
        reasons.push("no serve backend found".to_string());
    }

    if value(context, arguments.brokers.as_deref(), "KAFKA_BROKERS").is_none() {
        reasons.push("missing KAFKA_BROKERS".to_string());
    }

    let enabled_sources = value(
        context,
        arguments.enabled_sources.as_deref(),
        "RELAY_ENABLED_SOURCES",
    )
    .unwrap_or_else(|| "github,linear".to_string());
    if enabled_sources.trim().is_empty() {
        reasons.push("missing RELAY_ENABLED_SOURCES".to_string());
    }

    for source in parse_csv_lower(&enabled_sources) {
        match source.as_str() {
            "github" => {
                if value(context, None, "HMAC_SECRET_GITHUB").is_none() {
                    reasons.push("missing HMAC_SECRET_GITHUB for source github".to_string());
                }
            }
            "linear" => {
                if value(context, None, "HMAC_SECRET_LINEAR").is_none() {
                    reasons.push("missing HMAC_SECRET_LINEAR for source linear".to_string());
                }
            }
            "example" => {
                if value(context, None, "HMAC_SECRET_EXAMPLE").is_none() {
                    reasons.push("missing HMAC_SECRET_EXAMPLE for source example".to_string());
                }
            }
            _ => {}
        }
    }

    if !reasons.is_empty() && !context.global.force {
        return Err(anyhow!(
            "serve unavailable: {}. use --force to bypass",
            reasons.join("; ")
        ));
    }

    let mut contract_overrides = Vec::new();
    if let Some(contract) = context.contract.as_ref() {
        let active = resolve_active_serve_contract(context, contract)?;
        if let Some(bind) = active.bind {
            contract_overrides.push(("RELAY_BIND".to_string(), bind));
        }
        if !active.routes.is_empty() {
            contract_overrides.push((
                "RELAY_SERVE_ROUTES_JSON".to_string(),
                serde_json::to_string(&active.routes)?,
            ));
        }
        if !active.ingress_adapters.is_empty() {
            contract_overrides.push((
                "RELAY_INGRESS_ADAPTERS_JSON".to_string(),
                serde_json::to_string(&active.ingress_adapters)?,
            ));
        }
        if let Some(adapter_id) = active.ingress_adapter_id {
            contract_overrides.push(("RELAY_INGRESS_ADAPTER_ID".to_string(), adapter_id));
        }
        if !active.enabled_sources.is_empty() {
            contract_overrides.push((
                "RELAY_ENABLED_SOURCES".to_string(),
                active
                    .enabled_sources
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
    }

    let backend_spec = backend.ok_or_else(|| anyhow!("no serve backend resolved"))?;

    let mut overrides = contract_overrides;
    if let Some(bind) = &arguments.bind {
        overrides.push(("RELAY_BIND".to_string(), bind.clone()));
    }
    if let Some(enabled_sources) = &arguments.enabled_sources {
        overrides.push(("RELAY_ENABLED_SOURCES".to_string(), enabled_sources.clone()));
    }
    if let Some(brokers) = &arguments.brokers {
        overrides.push(("KAFKA_BROKERS".to_string(), brokers.clone()));
    }
    if let Some(prefix) = &arguments.source_topic_prefix {
        overrides.push(("RELAY_SOURCE_TOPIC_PREFIX".to_string(), prefix.clone()));
    }
    if let Some(instance_id) = &arguments.instance_id {
        overrides.push(("HOOK_INSTANCE_ID".to_string(), instance_id.clone()));
        overrides.push(("KAFKA_CLIENT_ID".to_string(), instance_id.clone()));
    }

    run_shell_backend(context, &backend_spec, &overrides)
}

#[derive(Debug, Clone)]
struct ActiveServeContract {
    ingress_adapters: Vec<ServeIngressAdapterEnv>,
    ingress_adapter_id: Option<String>,
    bind: Option<String>,
    routes: Vec<ServeRouteEnv>,
    enabled_sources: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
enum ServeIngressAdapterEnv {
    HttpWebhookIngress {
        id: String,
        bind: String,
        path_template: String,
        plugins: Vec<ServePluginEnv>,
    },
    WebsocketIngress {
        id: String,
        path_template: String,
        auth_mode: String,
        token_env: Option<String>,
        plugins: Vec<ServePluginEnv>,
    },
    McpIngestExposed {
        id: String,
        tool_name: String,
        transport_driver: String,
        bind: String,
        auth_mode: String,
        token_env: Option<String>,
        max_payload_bytes: usize,
        path: String,
        plugins: Vec<ServePluginEnv>,
    },
    KafkaIngress {
        id: String,
        topics: Vec<String>,
        group_id: String,
        brokers: Option<String>,
        plugins: Vec<ServePluginEnv>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
enum ServePluginEnv {
    EventTypeAlias { from: String, to: String },
    RequirePayloadField { pointer: String },
    AddMetaFlag { flag: String },
}

#[derive(Debug, Clone, Serialize)]
struct ServeRouteEnv {
    id: String,
    source_match: String,
    event_type_pattern: String,
    target_topic: String,
}

fn resolve_active_serve_contract(
    context: &AppContext,
    contract: &AppContract,
) -> Result<ActiveServeContract> {
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
                    "serve contract validation failed:\n{}",
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
            return Ok(ActiveServeContract {
                ingress_adapters: Vec::new(),
                ingress_adapter_id: None,
                bind: None,
                routes: Vec::new(),
                enabled_sources: BTreeSet::new(),
            });
        }
    };

    let active_routes = contract
        .serve
        .routes
        .iter()
        .filter(|route| {
            validated_profile
                .serve_route_ids
                .iter()
                .any(|active_route| active_route == &route.id)
        })
        .map(|route| ServeRouteEnv {
            id: route.id.clone(),
            source_match: route.source_match.clone(),
            event_type_pattern: route.event_type_pattern.clone(),
            target_topic: route.target_topic.clone(),
        })
        .collect::<Vec<_>>();

    let ingress_adapters = contract
        .serve
        .ingress_adapters
        .iter()
        .filter(|adapter| {
            validated_profile
                .serve_adapter_ids
                .iter()
                .any(|active_adapter_id| active_adapter_id == &adapter.id)
        })
        .map(to_serve_adapter_env)
        .collect::<Result<Vec<_>>>()?;

    let bind = ingress_adapters.iter().find_map(|adapter| match adapter {
        ServeIngressAdapterEnv::HttpWebhookIngress { bind, .. } => Some(bind.clone()),
        ServeIngressAdapterEnv::McpIngestExposed { bind, .. } => Some(bind.clone()),
        _ => None,
    });
    let ingress_adapter_id = ingress_adapters
        .first()
        .map(adapter_id)
        .map(ToString::to_string);

    let enabled_sources = active_routes
        .iter()
        .filter_map(|route| {
            let source = route.source_match.trim();
            if source.is_empty() || source == "*" || source.contains('*') {
                None
            } else {
                Some(source.to_ascii_lowercase())
            }
        })
        .collect::<BTreeSet<_>>();

    Ok(ActiveServeContract {
        ingress_adapters,
        ingress_adapter_id,
        bind,
        routes: active_routes,
        enabled_sources,
    })
}

fn to_serve_adapter_env(
    adapter: &relay_core::contract::IngressAdapter,
) -> Result<ServeIngressAdapterEnv> {
    let plugins = parse_plugins_from_config(&adapter.config, &adapter.id)?;
    match adapter.driver {
        IngressDriver::HttpWebhookIngress => Ok(ServeIngressAdapterEnv::HttpWebhookIngress {
            id: adapter.id.clone(),
            bind: required_string_config(&adapter.config, "bind", &adapter.id)?,
            path_template: optional_string_config(&adapter.config, "path_template")
                .unwrap_or_else(|| "/webhook/{source}".to_string()),
            plugins,
        }),
        IngressDriver::WebsocketIngress => Ok(ServeIngressAdapterEnv::WebsocketIngress {
            id: adapter.id.clone(),
            path_template: optional_string_config(&adapter.config, "path_template")
                .unwrap_or_else(|| "/ingest/ws/{source}".to_string()),
            auth_mode: required_string_config(&adapter.config, "auth_mode", &adapter.id)?,
            token_env: optional_string_config(&adapter.config, "token_env"),
            plugins,
        }),
        IngressDriver::McpIngestExposed => {
            let tool_name = optional_string_config(&adapter.config, "tool_name")
                .unwrap_or_else(|| "serve_ingest_event".to_string());
            let path = optional_string_config(&adapter.config, "path")
                .unwrap_or_else(|| format!("/mcp/tools/{tool_name}"));
            Ok(ServeIngressAdapterEnv::McpIngestExposed {
                id: adapter.id.clone(),
                tool_name,
                transport_driver: required_string_config(
                    &adapter.config,
                    "transport_driver",
                    &adapter.id,
                )?,
                bind: required_string_config(&adapter.config, "bind", &adapter.id)?,
                auth_mode: required_string_config(&adapter.config, "auth_mode", &adapter.id)?,
                token_env: optional_string_config(&adapter.config, "token_env"),
                max_payload_bytes: required_usize_config(
                    &adapter.config,
                    "max_payload_bytes",
                    &adapter.id,
                )?,
                path,
                plugins,
            })
        }
        IngressDriver::KafkaIngress => Ok(ServeIngressAdapterEnv::KafkaIngress {
            id: adapter.id.clone(),
            topics: required_string_array_config(&adapter.config, "topics", &adapter.id)?,
            group_id: required_string_config(&adapter.config, "group_id", &adapter.id)?,
            brokers: optional_string_config(&adapter.config, "brokers"),
            plugins,
        }),
        IngressDriver::Unknown(_) => Err(anyhow!(
            "active ingress adapter '{}' uses unsupported driver '{}'",
            adapter.id,
            adapter.driver.as_str()
        )),
    }
}

fn adapter_id(adapter: &ServeIngressAdapterEnv) -> &str {
    match adapter {
        ServeIngressAdapterEnv::HttpWebhookIngress { id, .. }
        | ServeIngressAdapterEnv::WebsocketIngress { id, .. }
        | ServeIngressAdapterEnv::McpIngestExposed { id, .. }
        | ServeIngressAdapterEnv::KafkaIngress { id, .. } => id.as_str(),
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
    config: &std::collections::BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<String> {
    match config.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Some(_) => Err(anyhow!(
            "ingress adapter '{}' key '{}' must be a non-empty string",
            adapter_id,
            key
        )),
        None => Err(anyhow!(
            "ingress adapter '{}' missing required key '{}'",
            adapter_id,
            key
        )),
    }
}

fn optional_string_config(
    config: &std::collections::BTreeMap<String, Value>,
    key: &str,
) -> Option<String> {
    match config.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn parse_plugins_from_config(
    config: &std::collections::BTreeMap<String, Value>,
    adapter_id: &str,
) -> Result<Vec<ServePluginEnv>> {
    let Some(value) = config.get("plugins") else {
        return Ok(Vec::new());
    };

    let Value::Array(items) = value else {
        return Err(anyhow!(
            "ingress adapter '{}' key 'plugins' must be an array",
            adapter_id
        ));
    };

    let mut plugins = Vec::with_capacity(items.len());
    for item in items {
        let plugin = item.clone().try_into::<ServePluginEnv>().map_err(|error| {
            anyhow!(
                "ingress adapter '{}' has invalid plugin config: {}",
                adapter_id,
                error
            )
        })?;
        plugins.push(plugin);
    }
    Ok(plugins)
}

fn required_usize_config(
    config: &std::collections::BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<usize> {
    match config.get(key) {
        Some(Value::Integer(value)) if *value >= 0 => Ok(*value as usize),
        Some(_) => Err(anyhow!(
            "ingress adapter '{}' key '{}' must be a non-negative integer",
            adapter_id,
            key
        )),
        None => Err(anyhow!(
            "ingress adapter '{}' missing required key '{}'",
            adapter_id,
            key
        )),
    }
}

fn required_string_array_config(
    config: &std::collections::BTreeMap<String, Value>,
    key: &str,
    adapter_id: &str,
) -> Result<Vec<String>> {
    let Some(value) = config.get(key) else {
        return Err(anyhow!(
            "ingress adapter '{}' missing required key '{}'",
            adapter_id,
            key
        ));
    };

    let Value::Array(items) = value else {
        return Err(anyhow!(
            "ingress adapter '{}' key '{}' must be an array of strings",
            adapter_id,
            key
        ));
    };

    let values = items
        .iter()
        .filter_map(|item| match item {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(anyhow!(
            "ingress adapter '{}' key '{}' must contain at least one topic",
            adapter_id,
            key
        ));
    }

    Ok(values)
}

fn parse_csv_lower(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

pub fn run_shell_backend(
    context: &AppContext,
    spec: &str,
    overrides: &[(String, String)],
) -> Result<()> {
    let mut command = Command::new("sh");
    command
        .arg("-lc")
        .arg(spec)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit());

    if let Some(repo_root) = &context.repo_root {
        command.current_dir(repo_root);
    }

    for (key, value) in context.merged_env_for_command(overrides) {
        command.env(key, value);
    }

    let status = command.status()?;
    if !status.success() {
        return Err(anyhow!("backend command failed: {}", spec));
    }

    Ok(())
}
