use crate::config::AppContext;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Serve,
    Relay,
    Smash,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleCapability {
    pub role: Role,
    pub available: bool,
    pub backend: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityReport {
    pub repo_root: Option<String>,
    pub roles: Vec<RoleCapability>,
    pub tools: BTreeMap<String, bool>,
}

pub fn detect_capabilities(context: &AppContext) -> CapabilityReport {
    let serve = detect_serve_capability(context);
    let relay = detect_relay_capability(context);
    let smash = detect_smash_capability(context);

    let mut tools = BTreeMap::new();
    for tool in [
        "docker",
        "journalctl",
        "systemctl",
        "cargo",
        "firecracker",
        "bash",
    ] {
        tools.insert(tool.to_string(), executable_exists(tool));
    }

    CapabilityReport {
        repo_root: context
            .repo_root
            .as_ref()
            .map(|path| path.display().to_string()),
        roles: vec![serve, relay, smash],
        tools,
    }
}

pub fn detect_serve_capability(context: &AppContext) -> RoleCapability {
    let mut reasons = Vec::new();

    let backend = resolve_serve_backend(context);
    if backend.is_none() {
        reasons.push("no serve backend found (webhook-relay/cargo fallback)".to_string());
    }

    if context.resolve_value(None, "KAFKA_BROKERS").is_none() {
        reasons.push("missing KAFKA_BROKERS".to_string());
    }

    let sources = context
        .resolve_value(None, "RELAY_ENABLED_SOURCES")
        .unwrap_or_else(|| "github,linear".to_string());
    if sources.trim().is_empty() {
        reasons.push("missing RELAY_ENABLED_SOURCES".to_string());
    }

    let enabled_sources = parse_csv_lower(&sources);
    for source in enabled_sources {
        match source.as_str() {
            "github" => {
                if context.resolve_value(None, "HMAC_SECRET_GITHUB").is_none() {
                    reasons
                        .push("missing HMAC_SECRET_GITHUB for enabled source github".to_string());
                }
            }
            "linear" => {
                if context.resolve_value(None, "HMAC_SECRET_LINEAR").is_none() {
                    reasons
                        .push("missing HMAC_SECRET_LINEAR for enabled source linear".to_string());
                }
            }
            "example" => {
                if context.resolve_value(None, "HMAC_SECRET_EXAMPLE").is_none() {
                    reasons
                        .push("missing HMAC_SECRET_EXAMPLE for enabled source example".to_string());
                }
            }
            _ => {}
        }
    }

    apply_profile_role_restriction(context, Role::Serve, &mut reasons);

    RoleCapability {
        role: Role::Serve,
        available: reasons.is_empty(),
        backend,
        reasons,
    }
}

pub fn detect_relay_capability(context: &AppContext) -> RoleCapability {
    let mut reasons = Vec::new();

    if context.resolve_value(None, "KAFKA_BROKERS").is_none() {
        reasons.push("missing KAFKA_BROKERS".to_string());
    }
    if context.resolve_value(None, "KAFKA_TOPICS").is_none() {
        reasons.push("missing KAFKA_TOPICS".to_string());
    }
    if context
        .resolve_value(None, "HOOK_RELAY_OUTPUT_TOPIC")
        .is_none()
    {
        reasons.push("missing HOOK_RELAY_OUTPUT_TOPIC".to_string());
    }

    apply_profile_role_restriction(context, Role::Relay, &mut reasons);

    RoleCapability {
        role: Role::Relay,
        available: reasons.is_empty(),
        backend: Some("internal".to_string()),
        reasons,
    }
}

pub fn detect_smash_capability(context: &AppContext) -> RoleCapability {
    let mut reasons = Vec::new();

    let backend = resolve_smash_backend(context);
    if backend.is_none() {
        reasons.push("no smash backend found (kafka-openclaw-hook/cargo fallback)".to_string());
    }

    if context.resolve_value(None, "KAFKA_BROKERS").is_none() {
        reasons.push("missing KAFKA_BROKERS".to_string());
    }
    if context
        .resolve_value(None, "OPENCLAW_WEBHOOK_URL")
        .is_none()
    {
        reasons.push("missing OPENCLAW_WEBHOOK_URL".to_string());
    }
    if context
        .resolve_value(None, "OPENCLAW_WEBHOOK_TOKEN")
        .is_none()
    {
        reasons.push("missing OPENCLAW_WEBHOOK_TOKEN".to_string());
    }

    apply_profile_role_restriction(context, Role::Smash, &mut reasons);

    RoleCapability {
        role: Role::Smash,
        available: reasons.is_empty(),
        backend,
        reasons,
    }
}

pub fn resolve_serve_backend(context: &AppContext) -> Option<String> {
    if let Some(command) = context
        .resolve_value(None, "HOOK_BACKEND_SERVE_CMD")
        .or_else(|| context.profile.backends.serve_cmd.clone())
    {
        if command_spec_runnable(&command) {
            return Some(command);
        }
    }

    if executable_exists("webhook-relay") {
        return Some("webhook-relay".to_string());
    }

    if let Some(repo_root) = &context.repo_root {
        let local_binary = repo_root.join("target/release/webhook-relay");
        if is_executable_file(&local_binary) {
            return Some(local_binary.display().to_string());
        }

        if executable_exists("cargo") && repo_root.join("src/main.rs").exists() {
            return Some(format!(
                "cargo run --manifest-path {} -p webhook-relay --release",
                repo_root.join("Cargo.toml").display()
            ));
        }
    }

    None
}

pub fn resolve_smash_backend(context: &AppContext) -> Option<String> {
    if let Some(command) = context
        .resolve_value(None, "HOOK_BACKEND_SMASH_CMD")
        .or_else(|| context.profile.backends.smash_cmd.clone())
    {
        if command_spec_runnable(&command) {
            return Some(command);
        }
    }

    if executable_exists("kafka-openclaw-hook") {
        return Some("kafka-openclaw-hook".to_string());
    }

    if let Some(repo_root) = &context.repo_root {
        let local_binary = repo_root.join("target/release/kafka-openclaw-hook");
        if is_executable_file(&local_binary) {
            return Some(local_binary.display().to_string());
        }

        if executable_exists("cargo")
            && repo_root
                .join("apps/kafka-openclaw-hook/src/main.rs")
                .exists()
        {
            return Some(format!(
                "cargo run --manifest-path {} -p kafka-openclaw-hook --release",
                repo_root.join("Cargo.toml").display()
            ));
        }
    }

    None
}

pub fn executable_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

pub fn command_spec_runnable(spec: &str) -> bool {
    if spec.trim().is_empty() {
        return false;
    }

    let first_token = spec.split_whitespace().next().unwrap_or("");
    if first_token.is_empty() {
        return false;
    }

    let candidate_path = PathBuf::from(first_token);
    if candidate_path.is_absolute() || first_token.contains('/') {
        return is_executable_file(&candidate_path);
    }

    executable_exists(first_token)
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn parse_csv_lower(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn apply_profile_role_restriction(context: &AppContext, role: Role, reasons: &mut Vec<String>) {
    let roles = context.profile_roles();
    if roles.is_empty() {
        return;
    }

    let role_name = match role {
        Role::Serve => "serve",
        Role::Relay => "relay",
        Role::Smash => "smash",
    };

    if !roles.iter().any(|candidate| candidate == role_name) {
        reasons.push(format!("profile capabilities.roles excludes {}", role_name));
    }
}
