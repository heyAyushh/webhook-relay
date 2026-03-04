use anyhow::{Context, Result, anyhow};
use relay_core::model::WebhookEnvelope;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;

const DEFAULT_OUTPUT_TIMEOUT_SECONDS: u64 = 5;

#[derive(Clone)]
pub enum McpRuntimeTransport {
    StdioJsonrpc {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    HttpSse {
        url: String,
        auth_mode: String,
        auth_token: Option<String>,
    },
}

#[derive(Clone)]
pub struct McpToolOutputAdapter {
    tool_name: String,
    transport: McpRuntimeTransport,
}

impl McpToolOutputAdapter {
    pub fn new(tool_name: String, transport: McpRuntimeTransport) -> Self {
        Self {
            tool_name,
            transport,
        }
    }

    pub async fn call(&self, envelope: &WebhookEnvelope) -> Result<()> {
        match &self.transport {
            McpRuntimeTransport::HttpSse {
                url,
                auth_mode,
                auth_token,
            } => {
                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(DEFAULT_OUTPUT_TIMEOUT_SECONDS))
                    .build()
                    .context("build mcp http_sse client")?;
                let mut request = client.post(url).header("Content-Type", "application/json");
                if auth_mode != "none" {
                    let token = auth_token
                        .as_ref()
                        .ok_or_else(|| anyhow!("mcp http_sse auth token missing"))?;
                    request = request.header("Authorization", format!("Bearer {}", token));
                }
                let response = request
                    .json(&json!({
                        "tool": self.tool_name,
                        "arguments": {
                            "envelope": envelope,
                        }
                    }))
                    .send()
                    .await
                    .context("call mcp http_sse endpoint")?;
                if !response.status().is_success() {
                    return Err(anyhow!(
                        "mcp http_sse returned status {}",
                        response.status()
                    ));
                }
                Ok(())
            }
            McpRuntimeTransport::StdioJsonrpc { command, args, env } => {
                let mut process = TokioCommand::new(command);
                process
                    .args(args)
                    .envs(env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let mut child = process.spawn().with_context(|| {
                    format!(
                        "spawn mcp stdio_jsonrpc command '{} {}'",
                        command,
                        args.join(" ")
                    )
                })?;
                let Some(mut stdin) = child.stdin.take() else {
                    return Err(anyhow!("mcp stdio_jsonrpc child stdin unavailable"));
                };
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": "hook-smash",
                    "method": "tools/call",
                    "params": {
                        "name": self.tool_name,
                        "arguments": {
                            "envelope": envelope,
                        }
                    }
                })
                .to_string();
                stdin
                    .write_all(payload.as_bytes())
                    .await
                    .context("write mcp stdio_jsonrpc payload")?;
                stdin
                    .write_all(b"\n")
                    .await
                    .context("write mcp stdio_jsonrpc newline")?;
                drop(stdin);

                let status = timeout(
                    Duration::from_secs(DEFAULT_OUTPUT_TIMEOUT_SECONDS),
                    child.wait(),
                )
                .await
                .context("mcp stdio_jsonrpc process timeout")??;
                if !status.success() {
                    return Err(anyhow!("mcp stdio_jsonrpc process exited with {}", status));
                }
                Ok(())
            }
        }
    }
}
