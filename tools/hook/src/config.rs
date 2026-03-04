use crate::cli::GlobalArgs;
use anyhow::{Context, Result, anyhow};
use dirs::config_dir;
use relay_core::contract::{AppContract, ValidationMode, parse_contract};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppContext {
    pub global: GlobalArgs,
    pub repo_root: Option<PathBuf>,
    pub profile_path: PathBuf,
    pub profile: HookProfile,
    pub env_file_values: BTreeMap<String, String>,
    pub contract_path: Option<PathBuf>,
    pub contract: Option<AppContract>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HookProfile {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub backends: BackendProfile,
    #[serde(default)]
    pub capabilities: CapabilityProfile,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BackendProfile {
    pub serve_cmd: Option<String>,
    pub smash_cmd: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CapabilityProfile {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl AppContext {
    pub fn load_with_contract(global: GlobalArgs, load_contract_config: bool) -> Result<Self> {
        let repo_root = discover_repo_root();
        let profile_path = resolve_profile_path(&global.profile, global.config.as_ref())?;
        let profile = load_profile_from_path(&profile_path)?;
        let env_file_values = merge_env_files(&global.env_files)?;
        let contract_resolution = if load_contract_config {
            resolve_contract(&global, repo_root.as_ref())?
        } else {
            ContractResolution {
                path: None,
                contract: None,
            }
        };

        Ok(Self {
            global,
            repo_root,
            profile_path,
            profile,
            env_file_values,
            contract_path: contract_resolution.path,
            contract: contract_resolution.contract,
        })
    }

    pub fn merged_env_for_command(
        &self,
        explicit_overrides: &[(String, String)],
    ) -> BTreeMap<String, String> {
        let mut merged = BTreeMap::new();

        for (key, value) in env::vars() {
            merged.insert(key, value);
        }
        for (key, value) in &self.env_file_values {
            merged.insert(key.clone(), value.clone());
        }
        for (key, value) in &self.profile.env {
            merged.insert(key.clone(), value.clone());
        }
        for (key, value) in explicit_overrides {
            merged.insert(key.clone(), value.clone());
        }

        merged.insert("RELAY_PROFILE".to_string(), self.global.profile.clone());
        merged.insert(
            "RELAY_VALIDATION_MODE".to_string(),
            self.resolved_validation_mode(),
        );
        if let Some(path) = self.contract_path.as_ref() {
            merged.insert(
                "RELAY_CONTRACT_PATH".to_string(),
                path.display().to_string(),
            );
        }

        merged
    }

    pub fn resolve_value(&self, flag_value: Option<&str>, env_key: &str) -> Option<String> {
        flag_value
            .map(ToString::to_string)
            .or_else(|| self.profile.env.get(env_key).cloned())
            .or_else(|| self.env_file_values.get(env_key).cloned())
            .or_else(|| env::var(env_key).ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    pub fn profile_roles(&self) -> Vec<String> {
        self.profile
            .capabilities
            .roles
            .iter()
            .map(|role| role.to_ascii_lowercase())
            .collect()
    }

    pub fn resolved_profile_text(&self) -> Result<String> {
        toml::to_string_pretty(&self.profile).context("serialize profile")
    }

    pub fn resolved_validation_mode(&self) -> String {
        if let Some(mode) = &self.global.validation_mode {
            return mode.trim().to_ascii_lowercase();
        }

        match self
            .contract
            .as_ref()
            .map(|contract| contract.policies.validation_mode)
        {
            Some(ValidationMode::Debug) => "debug".to_string(),
            _ => "strict".to_string(),
        }
    }

    pub fn redacted_merged_env_text(&self) -> String {
        let merged = self.merged_env_for_command(&[]);
        merged
            .into_iter()
            .map(|(key, value)| {
                let rendered = if is_sensitive_env_key(&key) {
                    "[REDACTED]".to_string()
                } else {
                    value
                };
                format!("{}={}", key, rendered)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct ContractResolution {
    pub path: Option<PathBuf>,
    pub contract: Option<AppContract>,
}

const EMBEDDED_DEFAULT_CONTRACT: &str = r#"
[app]
id = "default-openclaw"
name = "Default OpenClaw"
version = "1.0.0"

[policies]
allow_no_output = false
validation_mode = "strict"

[serve]

[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"
path_template = "/webhook/{source}"

[[serve.routes]]
id = "all-to-core"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]

[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"
token_env = "OPENCLAW_WEBHOOK_TOKEN"
timeout_seconds = 20
max_retries = 5

[[smash.routes]]
id = "core-to-openclaw"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "openclaw-output", required = true }]

[profiles.default-openclaw]
label = "Default OpenClaw"
serve_adapters = ["http-ingress"]
smash_adapters = ["openclaw-output"]
serve_routes = ["all-to-core"]
smash_routes = ["core-to-openclaw"]
"#;

pub fn resolve_contract(
    global: &GlobalArgs,
    repo_root: Option<&PathBuf>,
) -> Result<ContractResolution> {
    if let Some(path) = &global.contract {
        return Ok(ContractResolution {
            path: Some(path.clone()),
            contract: Some(load_contract_from_path(path)?),
        });
    }

    if let Some(app_id) = &global.app {
        let path = repo_root
            .cloned()
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .join("apps")
            .join(app_id)
            .join("contract.toml");
        if path.exists() {
            return Ok(ContractResolution {
                path: Some(path.clone()),
                contract: Some(load_contract_from_path(&path)?),
            });
        }
    }

    if let Ok(cwd) = env::current_dir() {
        let local_contract = cwd.join("contract.toml");
        if local_contract.exists() {
            return Ok(ContractResolution {
                path: Some(local_contract.clone()),
                contract: Some(load_contract_from_path(&local_contract)?),
            });
        }
    }

    Ok(ContractResolution {
        path: None,
        contract: Some(
            parse_contract(EMBEDDED_DEFAULT_CONTRACT).context("parse embedded contract")?,
        ),
    })
}

pub fn load_contract_from_path(path: &Path) -> Result<AppContract> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read contract file: {}", path.display()))?;
    parse_contract(&content).with_context(|| format!("parse contract TOML: {}", path.display()))
}

pub fn resolve_profile_path(profile_name: &str, explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.clone());
    }

    if profile_name.trim().is_empty() {
        return Err(anyhow!("profile name cannot be empty"));
    }

    if let Some(base) = config_dir() {
        return Ok(base
            .join("hook")
            .join("profiles")
            .join(format!("{}.toml", profile_name)));
    }

    Ok(PathBuf::from(".hook")
        .join("profiles")
        .join(format!("{}.toml", profile_name)))
}

pub fn load_profile_from_path(path: &Path) -> Result<HookProfile> {
    if !path.exists() {
        return Ok(HookProfile::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("read profile file: {}", path.display()))?;
    toml::from_str::<HookProfile>(&content)
        .with_context(|| format!("parse profile TOML: {}", path.display()))
}

pub fn write_profile(path: &Path, profile: &HookProfile) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("profile path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create profile directory: {}", parent.display()))?;

    let payload = toml::to_string_pretty(profile).context("serialize profile TOML")?;
    fs::write(path, payload).with_context(|| format!("write profile file: {}", path.display()))
}

pub fn merge_env_files(paths: &[PathBuf]) -> Result<BTreeMap<String, String>> {
    let mut merged = BTreeMap::new();
    for path in paths {
        let parsed = parse_env_file(path)?;
        for (key, value) in parsed {
            merged.insert(key, value);
        }
    }

    Ok(merged)
}

pub fn parse_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("read env file: {}", path.display()))?;

    let mut parsed = BTreeMap::new();
    for (line_number, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(anyhow!(
                "invalid env line at {}:{} (expected KEY=VALUE)",
                path.display(),
                line_number + 1
            ));
        };

        let key = raw_key.trim();
        if key.is_empty() {
            return Err(anyhow!(
                "empty env key at {}:{}",
                path.display(),
                line_number + 1
            ));
        }

        let value = raw_value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        parsed.insert(key.to_string(), value);
    }

    Ok(parsed)
}

pub fn discover_repo_root() -> Option<PathBuf> {
    if let Ok(configured_root) = env::var("HOOK_REPO_ROOT") {
        let configured_path = PathBuf::from(configured_root);
        if configured_path.join("Cargo.toml").exists() && configured_path.join("scripts").exists() {
            return Some(configured_path);
        }
    }

    let mut cursor = env::current_dir().ok()?;
    loop {
        if cursor.join("Cargo.toml").exists() && cursor.join("scripts").exists() {
            return Some(cursor);
        }

        if !cursor.pop() {
            break;
        }
    }

    None
}

fn is_sensitive_env_key(key: &str) -> bool {
    let normalized = key.to_ascii_uppercase();
    normalized.contains("SECRET")
        || normalized.contains("TOKEN")
        || normalized.contains("PASSWORD")
        || normalized.contains("PRIVATE_KEY")
        || normalized.contains("SASL")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::GlobalArgs;
    use std::path::PathBuf;

    #[test]
    fn profile_path_uses_name() {
        let path = resolve_profile_path("default", None).expect("path");
        assert!(path.to_string_lossy().contains("default.toml"));
    }

    #[test]
    fn parse_env_file_accepts_comments() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let env_path = temp_dir.path().join("test.env");
        fs::write(
            &env_path,
            "# comment\nKAFKA_BROKERS=127.0.0.1:9092\nOPENCLAW_WEBHOOK_TOKEN=abc\n",
        )
        .expect("write env");

        let parsed = parse_env_file(&env_path).expect("parse env");
        assert_eq!(
            parsed.get("KAFKA_BROKERS"),
            Some(&"127.0.0.1:9092".to_string())
        );
        assert_eq!(
            parsed.get("OPENCLAW_WEBHOOK_TOKEN"),
            Some(&"abc".to_string())
        );
    }

    #[test]
    fn resolve_contract_falls_back_to_embedded_default() {
        let global = GlobalArgs {
            profile: "default-openclaw".to_string(),
            app: None,
            contract: None,
            env_files: Vec::new(),
            config: None,
            force: false,
            json: false,
            validation_mode: None,
        };

        let resolved = resolve_contract(&global, None).expect("resolve contract");
        assert!(resolved.path.is_none());
        assert_eq!(
            resolved
                .contract
                .as_ref()
                .map(|contract| contract.app.id.as_str()),
            Some("default-openclaw")
        );
    }

    #[test]
    fn resolve_contract_uses_explicit_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let contract_path = temp_dir.path().join("contract.toml");
        fs::write(&contract_path, EMBEDDED_DEFAULT_CONTRACT).expect("write contract");

        let global = GlobalArgs {
            profile: "default-openclaw".to_string(),
            app: None,
            contract: Some(contract_path.clone()),
            env_files: Vec::new(),
            config: None,
            force: false,
            json: false,
            validation_mode: None,
        };
        let resolved = resolve_contract(&global, None).expect("resolve explicit contract");
        assert_eq!(resolved.path.as_deref(), Some(contract_path.as_path()));
    }

    #[test]
    fn resolved_validation_mode_prefers_cli_override() {
        let context = AppContext {
            global: GlobalArgs {
                profile: "default-openclaw".to_string(),
                app: None,
                contract: None,
                env_files: Vec::new(),
                config: None,
                force: false,
                json: false,
                validation_mode: Some("debug".to_string()),
            },
            repo_root: None,
            profile_path: PathBuf::from("default.toml"),
            profile: HookProfile::default(),
            env_file_values: BTreeMap::new(),
            contract_path: None,
            contract: Some(parse_contract(EMBEDDED_DEFAULT_CONTRACT).expect("parse embedded")),
        };

        assert_eq!(context.resolved_validation_mode(), "debug");
    }
}
