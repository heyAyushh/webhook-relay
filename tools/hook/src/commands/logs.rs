use crate::capabilities::detect_capabilities;
use crate::cli::{LogsArgs, LogsCollectArgs, LogsCommand, LogsFormat, LogsScope, LogsTailArgs};
use crate::config::AppContext;
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
struct LogSourceAvailability {
    name: String,
    available: bool,
    reason: String,
}

#[derive(Debug, Clone)]
struct CollectedEntry {
    file_name: String,
    source: String,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestSource {
    name: String,
    file: Option<String>,
    success: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Manifest {
    created_at: String,
    scope: LogsScope,
    format: LogsFormat,
    redact: bool,
    profile: String,
    sources: Vec<ManifestSource>,
}

pub async fn run(context: &AppContext, arguments: &LogsArgs) -> Result<()> {
    match &arguments.command {
        LogsCommand::Collect(details) => collect_logs(context, details),
        LogsCommand::Sources => show_sources(context),
        LogsCommand::Tail(details) => tail_logs(context, details),
    }
}

fn show_sources(context: &AppContext) -> Result<()> {
    let sources = detect_sources(context);
    if context.global.json {
        println!("{}", serde_json::to_string_pretty(&sources)?);
        return Ok(());
    }

    for source in sources {
        println!(
            "source={} available={} reason={}",
            source.name, source.available, source.reason
        );
    }

    Ok(())
}

fn tail_logs(context: &AppContext, arguments: &LogsTailArgs) -> Result<()> {
    if arguments.follow && which::which("journalctl").is_ok() {
        let mut command = Command::new("journalctl");
        command
            .arg("-f")
            .arg("-u")
            .arg("webhook-relay.service")
            .arg("-u")
            .arg("kafka-openclaw-hook.service")
            .arg("-u")
            .arg("firecracker@relay.service")
            .arg("--no-pager")
            .arg("-n")
            .arg(arguments.lines.to_string());

        let status = command.status().context("tail journalctl logs")?;
        if !status.success() {
            return Err(anyhow!("journalctl tail failed"));
        }

        return Ok(());
    }

    let collect_arguments = LogsCollectArgs {
        scope: arguments.scope.clone(),
        format: LogsFormat::Stream,
        since: None,
        until: None,
        lines: arguments.lines,
        output: None,
        redact: arguments.redact,
        no_redact: arguments.no_redact,
    };

    collect_logs(context, &collect_arguments)
}

fn collect_logs(context: &AppContext, arguments: &LogsCollectArgs) -> Result<()> {
    let redact = if arguments.no_redact {
        false
    } else {
        arguments.redact
    };

    let mut entries = Vec::new();
    let mut manifest_sources = Vec::new();

    push_content(
        &mut entries,
        &mut manifest_sources,
        "diagnostics/capabilities.json",
        "capabilities",
        serde_json::to_string_pretty(&detect_capabilities(context))?,
    );

    push_content(
        &mut entries,
        &mut manifest_sources,
        "diagnostics/env.txt",
        "env",
        context.redacted_merged_env_text(),
    );

    collect_journal_logs(context, arguments, &mut entries, &mut manifest_sources)?;
    collect_docker_logs(context, arguments, &mut entries, &mut manifest_sources)?;
    collect_firecracker_logs(context, &mut entries, &mut manifest_sources)?;

    if redact {
        for entry in &mut entries {
            entry.content = redact_content(&entry.content)?;
        }
    }

    if matches!(arguments.format, LogsFormat::Stream | LogsFormat::Both) {
        for entry in &entries {
            println!("===== {} ({}) =====", entry.source, entry.file_name);
            println!("{}", entry.content);
            println!();
        }
    }

    if matches!(arguments.format, LogsFormat::Bundle | LogsFormat::Both) {
        let output_path = default_output_path(arguments.output.as_ref());
        let manifest = Manifest {
            created_at: Utc::now().to_rfc3339(),
            scope: arguments.scope.clone(),
            format: arguments.format.clone(),
            redact,
            profile: context.global.profile.clone(),
            sources: manifest_sources,
        };
        write_bundle(&output_path, &entries, &manifest)?;
        println!("log bundle written to {}", output_path.display());
    }

    Ok(())
}

fn collect_journal_logs(
    context: &AppContext,
    arguments: &LogsCollectArgs,
    entries: &mut Vec<CollectedEntry>,
    manifest_sources: &mut Vec<ManifestSource>,
) -> Result<()> {
    let scope_matches = matches!(
        arguments.scope,
        LogsScope::Auto | LogsScope::Full | LogsScope::Runtime | LogsScope::System
    );
    if !scope_matches {
        return Ok(());
    }

    if which::which("journalctl").is_err() {
        manifest_sources.push(ManifestSource {
            name: "journalctl".to_string(),
            file: None,
            success: false,
            error: Some("journalctl not available".to_string()),
        });
        return Ok(());
    }

    for unit in [
        "webhook-relay.service",
        "kafka-openclaw-hook.service",
        "firecracker@relay.service",
    ] {
        let mut command = Command::new("journalctl");
        command
            .arg("-u")
            .arg(unit)
            .arg("--no-pager")
            .arg("-n")
            .arg(arguments.lines.to_string())
            .arg("-o")
            .arg("short-iso");

        if let Some(since) = &arguments.since {
            command.arg("--since").arg(since);
        }
        if let Some(until) = &arguments.until {
            command.arg("--until").arg(until);
        }

        match command.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if output.status.success() {
                    push_content(
                        entries,
                        manifest_sources,
                        &format!("journal/{}.log", sanitize_file_name(unit)),
                        &format!("journalctl:{}", unit),
                        stdout,
                    );
                } else {
                    manifest_sources.push(ManifestSource {
                        name: format!("journalctl:{}", unit),
                        file: None,
                        success: false,
                        error: Some(stderr),
                    });
                }
            }
            Err(error) => {
                manifest_sources.push(ManifestSource {
                    name: format!("journalctl:{}", unit),
                    file: None,
                    success: false,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    if context.repo_root.is_none() {
        manifest_sources.push(ManifestSource {
            name: "journalctl-context".to_string(),
            file: None,
            success: false,
            error: Some("repo root not detected".to_string()),
        });
    }

    Ok(())
}

fn collect_docker_logs(
    context: &AppContext,
    arguments: &LogsCollectArgs,
    entries: &mut Vec<CollectedEntry>,
    manifest_sources: &mut Vec<ManifestSource>,
) -> Result<()> {
    if !matches!(
        arguments.scope,
        LogsScope::Auto | LogsScope::Full | LogsScope::Runtime
    ) {
        return Ok(());
    }

    let Some(repo_root) = &context.repo_root else {
        manifest_sources.push(ManifestSource {
            name: "docker".to_string(),
            file: None,
            success: false,
            error: Some("repo root not detected".to_string()),
        });
        return Ok(());
    };

    if which::which("docker").is_err() {
        manifest_sources.push(ManifestSource {
            name: "docker".to_string(),
            file: None,
            success: false,
            error: Some("docker not available".to_string()),
        });
        return Ok(());
    }

    let mut command = Command::new("docker");
    command
        .arg("compose")
        .arg("-f")
        .arg(repo_root.join("docker-compose.yml"));

    if repo_root.join("docker-compose.dev.yml").exists() {
        command
            .arg("-f")
            .arg(repo_root.join("docker-compose.dev.yml"));
    }

    command
        .arg("logs")
        .arg("--no-color")
        .arg("--tail")
        .arg(arguments.lines.to_string());

    match command.output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if output.status.success() {
                push_content(
                    entries,
                    manifest_sources,
                    "docker/compose.log",
                    "docker-compose",
                    stdout,
                );
            } else {
                manifest_sources.push(ManifestSource {
                    name: "docker-compose".to_string(),
                    file: None,
                    success: false,
                    error: Some(stderr),
                });
            }
        }
        Err(error) => {
            manifest_sources.push(ManifestSource {
                name: "docker-compose".to_string(),
                file: None,
                success: false,
                error: Some(error.to_string()),
            });
        }
    }

    Ok(())
}

fn collect_firecracker_logs(
    context: &AppContext,
    entries: &mut Vec<CollectedEntry>,
    manifest_sources: &mut Vec<ManifestSource>,
) -> Result<()> {
    let mut file_paths = vec![
        PathBuf::from("/var/log/firecracker/watchdog/heartbeat.log"),
        PathBuf::from("/var/log/firecracker/watchdog/boot.log"),
        PathBuf::from("/var/log/firecracker/watchdog/shutdown.log"),
        PathBuf::from("/tmp/firecracker-watchdog/heartbeat.log"),
        PathBuf::from("/tmp/firecracker-watchdog/boot.log"),
        PathBuf::from("/tmp/firecracker-watchdog/shutdown.log"),
    ];

    if let Some(repo_root) = &context.repo_root {
        file_paths.push(repo_root.join("firecracker/watchdog/status.sh"));
    }

    for path in file_paths {
        if !path.exists() {
            continue;
        }

        if path.extension().map(|value| value == "sh").unwrap_or(false) {
            let output = Command::new("bash").arg(&path).output();
            match output {
                Ok(output) => {
                    let content = String::from_utf8_lossy(&output.stdout).to_string();
                    push_content(
                        entries,
                        manifest_sources,
                        "firecracker/status.log",
                        &format!("script:{}", path.display()),
                        content,
                    );
                }
                Err(error) => {
                    manifest_sources.push(ManifestSource {
                        name: format!("script:{}", path.display()),
                        file: None,
                        success: false,
                        error: Some(error.to_string()),
                    });
                }
            }
            continue;
        }

        match fs::read_to_string(&path) {
            Ok(content) => {
                push_content(
                    entries,
                    manifest_sources,
                    &format!(
                        "firecracker/{}.log",
                        sanitize_file_name(&path.display().to_string())
                    ),
                    &format!("file:{}", path.display()),
                    content,
                );
            }
            Err(error) => {
                manifest_sources.push(ManifestSource {
                    name: format!("file:{}", path.display()),
                    file: None,
                    success: false,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    Ok(())
}

fn detect_sources(context: &AppContext) -> Vec<LogSourceAvailability> {
    vec![
        LogSourceAvailability {
            name: "capabilities".to_string(),
            available: true,
            reason: "always available".to_string(),
        },
        LogSourceAvailability {
            name: "env".to_string(),
            available: true,
            reason: "always available".to_string(),
        },
        LogSourceAvailability {
            name: "journalctl".to_string(),
            available: which::which("journalctl").is_ok(),
            reason: "systemd journal collector".to_string(),
        },
        LogSourceAvailability {
            name: "docker-compose".to_string(),
            available: which::which("docker").is_ok() && context.repo_root.is_some(),
            reason: "docker compose logs collector".to_string(),
        },
        LogSourceAvailability {
            name: "firecracker-files".to_string(),
            available: true,
            reason: "firecracker/watchdog file collector".to_string(),
        },
    ]
}

fn push_content(
    entries: &mut Vec<CollectedEntry>,
    manifest_sources: &mut Vec<ManifestSource>,
    file_name: &str,
    source: &str,
    content: String,
) {
    entries.push(CollectedEntry {
        file_name: file_name.to_string(),
        source: source.to_string(),
        content,
    });

    manifest_sources.push(ManifestSource {
        name: source.to_string(),
        file: Some(file_name.to_string()),
        success: true,
        error: None,
    });
}

fn sanitize_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn default_output_path(explicit: Option<&PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path.clone();
    }

    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    PathBuf::from(format!("hook-logs-{}.tar.gz", stamp))
}

fn write_bundle(output_path: &Path, entries: &[CollectedEntry], manifest: &Manifest) -> Result<()> {
    let staging_dir = std::env::temp_dir().join(format!(
        "hook-logs-staging-{}-{}",
        std::process::id(),
        Utc::now().timestamp_millis()
    ));
    fs::create_dir_all(&staging_dir)
        .with_context(|| format!("create staging dir: {}", staging_dir.display()))?;

    for entry in entries {
        let target_path = staging_dir.join(&entry.file_name);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create log parent dir: {}", parent.display()))?;
        }
        fs::write(&target_path, &entry.content)
            .with_context(|| format!("write log entry: {}", target_path.display()))?;
    }

    let manifest_path = staging_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(manifest)?)
        .with_context(|| format!("write manifest: {}", manifest_path.display()))?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create bundle parent: {}", parent.display()))?;
        }
    }

    let status = Command::new("tar")
        .arg("-czf")
        .arg(output_path)
        .arg("-C")
        .arg(&staging_dir)
        .arg(".")
        .status()
        .context("create tar.gz bundle")?;

    fs::remove_dir_all(&staging_dir).ok();

    if !status.success() {
        return Err(anyhow!(
            "tar failed while creating bundle at {}",
            output_path.display()
        ));
    }

    Ok(())
}

fn redact_content(content: &str) -> Result<String> {
    let mut redacted = content.to_string();

    let patterns = [
        Regex::new(
            r"(?im)\b([A-Z0-9_]*(SECRET|TOKEN|PASSWORD|PRIVATE_KEY|SASL)[A-Z0-9_]*)=([^\s]+)",
        )?,
        Regex::new(r"(?im)(authorization\s*:\s*bearer\s+)([^\s]+)")?,
        Regex::new(r"(?s)-----BEGIN [A-Z ]+PRIVATE KEY-----.*?-----END [A-Z ]+PRIVATE KEY-----")?,
    ];

    redacted = patterns[0]
        .replace_all(&redacted, "$1=[REDACTED]")
        .to_string();
    redacted = patterns[1]
        .replace_all(&redacted, "$1[REDACTED]")
        .to_string();
    redacted = patterns[2]
        .replace_all(&redacted, "[REDACTED_PRIVATE_KEY]")
        .to_string();

    Ok(redacted)
}
