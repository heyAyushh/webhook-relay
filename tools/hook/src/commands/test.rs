use crate::capabilities::detect_capabilities;
use crate::cli::{SmokeCommand, TestArgs, TestCommand};
use crate::config::AppContext;
use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use std::process::{Command, Stdio};

pub async fn run(context: &AppContext, arguments: &TestArgs) -> Result<()> {
    match &arguments.command {
        TestCommand::Env => run_env_test(context),
        TestCommand::Smoke(smoke) => match &smoke.command {
            SmokeCommand::Serve { passthrough } => run_smoke_serve(context, passthrough).await,
            SmokeCommand::Relay => run_smoke_relay(context),
            SmokeCommand::Smash => run_smoke_smash(context).await,
        },
    }
}

fn run_env_test(context: &AppContext) -> Result<()> {
    let report = detect_capabilities(context);
    if context.global.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for role in report.roles {
            println!(
                "role={} available={} reasons={}",
                serde_json::to_string(&role.role)?.trim_matches('"'),
                role.available,
                if role.reasons.is_empty() {
                    "none".to_string()
                } else {
                    role.reasons.join("; ")
                }
            );
        }
    }

    Ok(())
}

async fn run_smoke_serve(context: &AppContext, passthrough: &[String]) -> Result<()> {
    let smoke_options = parse_serve_smoke_options(passthrough)?;
    if !smoke_options.skip_unit_tests {
        let repo_root = context
            .repo_root
            .as_ref()
            .ok_or_else(|| anyhow!("repo root not detected; cannot run workspace tests"))?;
        let mut command = Command::new("cargo");
        command
            .arg("test")
            .arg("--workspace")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit())
            .current_dir(repo_root);
        let status = command
            .status()
            .context("run workspace tests for smoke serve")?;
        if !status.success() {
            return Err(anyhow!("serve smoke failed: cargo test --workspace"));
        }
    }

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(smoke_options.insecure)
        .build()
        .context("build smoke serve http client")?;

    let health = client
        .get(format!("{}/health", smoke_options.relay_url))
        .send()
        .await
        .context("request relay /health")?;
    if !health.status().is_success() {
        return Err(anyhow!(
            "serve smoke failed: /health returned {}",
            health.status()
        ));
    }

    let unauthorized = client
        .post(format!("{}/webhook/github", smoke_options.relay_url))
        .header("Content-Type", "application/json")
        .header("X-GitHub-Event", "pull_request")
        .header("X-GitHub-Delivery", "hook-smoke-unauthorized")
        .header("X-Hub-Signature-256", "sha256=deadbeef")
        .body("{\"action\":\"opened\"}")
        .send()
        .await
        .context("request unauthorized github webhook")?;
    if unauthorized.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow!(
            "serve smoke failed: unauthorized github webhook expected 401, got {}",
            unauthorized.status()
        ));
    }

    println!("serve smoke check passed (/health + unauthorized github signature)");
    Ok(())
}

fn run_smoke_relay(context: &AppContext) -> Result<()> {
    let brokers = context.resolve_value(None, "KAFKA_BROKERS");
    let topics = context.resolve_value(None, "KAFKA_TOPICS");
    let output_topic = context.resolve_value(None, "HOOK_RELAY_OUTPUT_TOPIC");

    if brokers.is_none() || topics.is_none() || output_topic.is_none() {
        return Err(anyhow!(
            "relay smoke check missing required values: KAFKA_BROKERS, KAFKA_TOPICS, HOOK_RELAY_OUTPUT_TOPIC"
        ));
    }

    println!("relay smoke check passed (required config detected)");
    Ok(())
}

async fn run_smoke_smash(context: &AppContext) -> Result<()> {
    let webhook_url = context
        .resolve_value(None, "OPENCLAW_WEBHOOK_URL")
        .ok_or_else(|| anyhow!("missing OPENCLAW_WEBHOOK_URL"))?;
    let token = context
        .resolve_value(None, "OPENCLAW_WEBHOOK_TOKEN")
        .ok_or_else(|| anyhow!("missing OPENCLAW_WEBHOOK_TOKEN"))?;

    let client = Client::new();
    let response = client
        .post(&webhook_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .context("send smash smoke request")?;

    println!("smash smoke response status={}", response.status());
    Ok(())
}

#[derive(Debug, Clone)]
struct ServeSmokeOptions {
    relay_url: String,
    insecure: bool,
    skip_unit_tests: bool,
}

fn parse_serve_smoke_options(arguments: &[String]) -> Result<ServeSmokeOptions> {
    let mut relay_url = "http://127.0.0.1:8080".to_string();
    let mut insecure = false;
    let mut skip_unit_tests = false;

    let mut index = 0usize;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--relay-url" => {
                let Some(next_value) = arguments.get(index + 1) else {
                    return Err(anyhow!("missing value for --relay-url"));
                };
                relay_url = next_value.clone();
                index = index.saturating_add(2);
            }
            "--insecure" => {
                insecure = true;
                index = index.saturating_add(1);
            }
            "--skip-unit-tests" => {
                skip_unit_tests = true;
                index = index.saturating_add(1);
            }
            unknown => {
                return Err(anyhow!(
                    "unsupported smoke serve arg '{}'; supported: --relay-url, --insecure, --skip-unit-tests",
                    unknown
                ));
            }
        }
    }

    Ok(ServeSmokeOptions {
        relay_url,
        insecure,
        skip_unit_tests,
    })
}
