mod kafka;
mod mcp;
mod openclaw;
mod websocket_client;
mod websocket_server;

use crate::smash::config::{Config, SmashAdapterConfig, SmashTransportConfig};
use anyhow::{Context, Result, anyhow};
use relay_core::model::WebhookEnvelope;
use std::collections::BTreeMap;
use std::env;

use kafka::KafkaOutputAdapter;
use mcp::{McpRuntimeTransport, McpToolOutputAdapter};
use openclaw::{OpenclawOutputAdapter, OpenclawOutputTarget};
use websocket_client::WebsocketClientOutputAdapter;
use websocket_server::WebsocketServerOutputAdapter;

#[derive(Clone)]
pub enum RuntimeAdapter {
    Openclaw(OpenclawOutputAdapter),
    KafkaOutput(KafkaOutputAdapter),
    WebsocketClient(WebsocketClientOutputAdapter),
    WebsocketServer(WebsocketServerOutputAdapter),
    McpTool(McpToolOutputAdapter),
}

impl RuntimeAdapter {
    pub async fn deliver(&self, adapter_id: &str, envelope: &WebhookEnvelope) -> Result<()> {
        match self {
            RuntimeAdapter::Openclaw(adapter) => adapter
                .forward_with_retry(envelope)
                .await
                .with_context(|| format!("forward via adapter '{}'", adapter_id)),
            RuntimeAdapter::KafkaOutput(adapter) => adapter
                .publish(envelope)
                .await
                .with_context(|| format!("kafka_output adapter '{}'", adapter_id)),
            RuntimeAdapter::WebsocketClient(adapter) => adapter
                .send(envelope)
                .await
                .with_context(|| format!("websocket_client_output adapter '{}'", adapter_id)),
            RuntimeAdapter::WebsocketServer(adapter) => adapter
                .broadcast(envelope)
                .await
                .with_context(|| format!("websocket_server_output adapter '{}'", adapter_id)),
            RuntimeAdapter::McpTool(adapter) => adapter
                .call(envelope)
                .await
                .with_context(|| format!("mcp_tool_output adapter '{}'", adapter_id)),
        }
    }
}

pub async fn build_runtime_adapters(config: &Config) -> Result<BTreeMap<String, RuntimeAdapter>> {
    let mut by_id: BTreeMap<String, RuntimeAdapter> = BTreeMap::new();
    let transport_map = config
        .transports
        .iter()
        .map(|transport| (transport_name(transport).to_string(), transport.clone()))
        .collect::<BTreeMap<_, _>>();

    for adapter in &config.adapters {
        let (id, runtime_adapter) = match adapter {
            SmashAdapterConfig::OpenclawHttpOutput {
                id,
                url,
                token_env,
                timeout_seconds,
                max_retries,
                ..
            } => {
                let token = required_env(token_env)?;
                let target = OpenclawOutputTarget {
                    adapter_id: id.clone(),
                    webhook_url: url.clone(),
                    webhook_token: token,
                    message_max_bytes: config.openclaw_message_max_bytes,
                    http_timeout_seconds: *timeout_seconds,
                    max_retries: *max_retries,
                    backoff_base_seconds: config.backoff_base_seconds,
                    backoff_max_seconds: config.backoff_max_seconds,
                };
                let output = OpenclawOutputAdapter::new(target)
                    .with_context(|| format!("initialize openclaw output adapter '{}'", id))?;
                (id.clone(), RuntimeAdapter::Openclaw(output))
            }
            SmashAdapterConfig::KafkaOutput {
                id,
                topic,
                key_mode,
                ..
            } => {
                let output =
                    KafkaOutputAdapter::from_config(config, topic.clone(), key_mode.clone())
                        .with_context(|| format!("initialize kafka_output adapter '{}'", id))?;
                (id.clone(), RuntimeAdapter::KafkaOutput(output))
            }
            SmashAdapterConfig::WebsocketClientOutput {
                id,
                url,
                auth_mode,
                token_env,
                send_timeout_ms,
                retry_max_retries,
                retry_backoff_ms,
                ..
            } => {
                let token = resolve_optional_auth_token(auth_mode, token_env.as_deref())?;
                let output = WebsocketClientOutputAdapter::new(
                    url.clone(),
                    auth_mode.clone(),
                    token,
                    *send_timeout_ms,
                    *retry_max_retries,
                    *retry_backoff_ms,
                );
                (id.clone(), RuntimeAdapter::WebsocketClient(output))
            }
            SmashAdapterConfig::WebsocketServerOutput {
                id,
                bind,
                path,
                auth_mode,
                token_env,
                max_clients,
                queue_depth_per_client,
                send_timeout_ms,
                ..
            } => {
                let token = resolve_optional_auth_token(auth_mode, token_env.as_deref())?;
                let output = WebsocketServerOutputAdapter::start(
                    id,
                    bind,
                    path,
                    auth_mode,
                    token,
                    *max_clients,
                    *queue_depth_per_client,
                    *send_timeout_ms,
                )
                .await
                .with_context(|| format!("initialize websocket_server_output adapter '{}'", id))?;
                (id.clone(), RuntimeAdapter::WebsocketServer(output))
            }
            SmashAdapterConfig::McpToolOutput {
                id,
                tool_name,
                transport_ref,
                ..
            } => {
                let Some(transport) = transport_map.get(transport_ref) else {
                    return Err(anyhow!(
                        "mcp_tool_output adapter '{}' references missing transport '{}'",
                        id,
                        transport_ref
                    ));
                };
                let runtime_transport = match transport {
                    SmashTransportConfig::StdioJsonrpc {
                        command, args, env, ..
                    } => McpRuntimeTransport::StdioJsonrpc {
                        command: command.clone(),
                        args: args.clone(),
                        env: env.clone(),
                    },
                    SmashTransportConfig::HttpSse {
                        url,
                        auth_mode,
                        token_env,
                        ..
                    } => McpRuntimeTransport::HttpSse {
                        url: url.clone(),
                        auth_mode: auth_mode.clone(),
                        auth_token: resolve_optional_auth_token(auth_mode, token_env.as_deref())?,
                    },
                };
                let output = McpToolOutputAdapter::new(tool_name.clone(), runtime_transport);
                (id.clone(), RuntimeAdapter::McpTool(output))
            }
        };

        if by_id.insert(id.clone(), runtime_adapter).is_some() {
            return Err(anyhow!("duplicate runtime adapter id '{}'", id));
        }
    }

    Ok(by_id)
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("env var {name} cannot be empty"));
    }
    Ok(value)
}

fn resolve_optional_auth_token(auth_mode: &str, token_env: Option<&str>) -> Result<Option<String>> {
    match auth_mode.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(None),
        "bearer" | "hmac" => {
            let token_env =
                token_env.ok_or_else(|| anyhow!("auth_mode '{}' requires token_env", auth_mode))?;
            let token = required_env(token_env)?;
            Ok(Some(token))
        }
        other => Err(anyhow!("unsupported auth_mode '{}'", other)),
    }
}

fn transport_name(transport: &SmashTransportConfig) -> &str {
    match transport {
        SmashTransportConfig::StdioJsonrpc { name, .. }
        | SmashTransportConfig::HttpSse { name, .. } => name.as_str(),
    }
}
