use crate::capabilities::detect_capabilities;
use crate::cli::{DebugArgs, DebugCommand};
use crate::config::AppContext;
use anyhow::Result;

pub async fn run(context: &AppContext, arguments: &DebugArgs) -> Result<()> {
    match &arguments.command {
        DebugCommand::Capabilities => {
            let report = detect_capabilities(context);
            if context.global.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }

            println!("profile: {}", context.global.profile);
            println!(
                "repo_root: {}",
                report.repo_root.as_deref().unwrap_or("not detected")
            );
            for role in report.roles {
                println!(
                    "role={} available={} backend={}{}",
                    serde_json::to_string(&role.role)?.trim_matches('"'),
                    role.available,
                    role.backend.as_deref().unwrap_or("none"),
                    if role.reasons.is_empty() {
                        "".to_string()
                    } else {
                        format!(" reasons={}", role.reasons.join("; "))
                    }
                );
            }

            for (tool, available) in report.tools {
                println!("tool={} available={}", tool, available);
            }
        }
        DebugCommand::Env { no_redact } => {
            let payload = if *no_redact {
                let merged = context.merged_env_for_command(&[]);
                merged
                    .into_iter()
                    .map(|(key, value)| format!("{}={}", key, value))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                context.redacted_merged_env_text()
            };
            println!("{}", payload);
        }
    }

    Ok(())
}
