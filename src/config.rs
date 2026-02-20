use anyhow::{Context, Result, anyhow};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub db_path: PathBuf,

    pub openclaw_gateway_url: String,
    pub openclaw_hooks_token: String,

    pub github_webhook_secret: String,
    pub linear_webhook_secret: String,
    pub linear_agent_user_id: Option<String>,

    pub dedup_retention_days: i64,
    pub github_cooldown_seconds: i64,
    pub linear_cooldown_seconds: i64,
    pub linear_timestamp_window_seconds: i64,
    pub linear_enforce_timestamp_check: bool,

    pub http_connect_timeout_seconds: u64,
    pub http_request_timeout_seconds: u64,
    pub forward_max_attempts: u32,
    pub forward_initial_backoff_seconds: u64,
    pub forward_max_backoff_seconds: u64,

    pub ingress_max_body_bytes: usize,
    pub queue_poll_interval_ms: u64,

    pub admin_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let openclaw_gateway_url = required_env("OPENCLAW_GATEWAY_URL")?;
        let openclaw_hooks_token = required_env("OPENCLAW_HOOKS_TOKEN")?;
        let github_webhook_secret = required_env("GITHUB_WEBHOOK_SECRET")?;
        let linear_webhook_secret = required_env("LINEAR_WEBHOOK_SECRET")?;

        let bind_addr =
            env::var("WEBHOOK_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:9000".to_string());
        let db_path = PathBuf::from(
            env::var("WEBHOOK_DB_PATH")
                .unwrap_or_else(|_| "/tmp/webhook-relay/relay.redb".to_string()),
        );

        Ok(Self {
            bind_addr,
            db_path,
            openclaw_gateway_url,
            openclaw_hooks_token,
            github_webhook_secret,
            linear_webhook_secret,
            linear_agent_user_id: optional_non_empty("LINEAR_AGENT_USER_ID"),
            dedup_retention_days: env_i64("WEBHOOK_DEDUP_RETENTION_DAYS", 7)?,
            github_cooldown_seconds: env_i64("GITHUB_COOLDOWN_SECONDS", 30)?,
            linear_cooldown_seconds: env_i64("LINEAR_COOLDOWN_SECONDS", 30)?,
            linear_timestamp_window_seconds: env_i64("LINEAR_TIMESTAMP_WINDOW_SECONDS", 60)?,
            linear_enforce_timestamp_check: env_bool("LINEAR_ENFORCE_TIMESTAMP_CHECK", true),
            http_connect_timeout_seconds: env_u64("WEBHOOK_CURL_CONNECT_TIMEOUT_SECONDS", 5)?,
            http_request_timeout_seconds: env_u64("WEBHOOK_CURL_MAX_TIME_SECONDS", 20)?,
            forward_max_attempts: env_u32("WEBHOOK_FORWARD_MAX_ATTEMPTS", 5)?,
            forward_initial_backoff_seconds: env_u64("WEBHOOK_FORWARD_INITIAL_BACKOFF_SECONDS", 1)?,
            forward_max_backoff_seconds: env_u64("WEBHOOK_FORWARD_MAX_BACKOFF_SECONDS", 30)?,
            ingress_max_body_bytes: env_usize("WEBHOOK_MAX_BODY_BYTES", 512 * 1024)?,
            queue_poll_interval_ms: env_u64("WEBHOOK_QUEUE_POLL_INTERVAL_MS", 500)?,
            admin_token: optional_non_empty("WEBHOOK_ADMIN_TOKEN"),
        })
    }

    pub fn dedup_retention_seconds(&self) -> i64 {
        self.dedup_retention_days.saturating_mul(24 * 60 * 60)
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("missing required env var: {name}"))?;
    if value.trim().is_empty() {
        return Err(anyhow!("required env var {name} cannot be empty"));
    }
    Ok(value)
}

fn optional_non_empty(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn env_bool(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.as_str(),
            "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
        ),
        Err(_) => default,
    }
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
