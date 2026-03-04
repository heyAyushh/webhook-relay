use crate::cli::{ConfigArgs, ConfigCommand, ConfigImportArgs};
use crate::config::{
    AppContext, HookProfile, load_profile_from_path, merge_env_files, parse_env_file, write_profile,
};
use anyhow::{Context, Result, anyhow};
use relay_core::contract::ValidationMode;
use relay_core::contract_validator::validate_contract;

pub async fn run(context: &AppContext, arguments: &ConfigArgs) -> Result<()> {
    match &arguments.command {
        ConfigCommand::Import(details) => import_profile(context, details),
        ConfigCommand::Show => show_profile(context),
        ConfigCommand::Validate => validate_profile(context),
    }
}

fn import_profile(context: &AppContext, arguments: &ConfigImportArgs) -> Result<()> {
    let mut profile = context.profile.clone();

    if let Some(toml_path) = &arguments.toml {
        let imported = load_profile_from_path(toml_path)?;
        merge_profile(&mut profile, &imported);
    }

    let mut env_file_paths = context.global.env_files.clone();
    env_file_paths.extend(arguments.env_files.clone());
    let imported_env = merge_env_files(&env_file_paths)?;
    for (key, value) in imported_env {
        profile.env.insert(key, value);
    }

    write_profile(&context.profile_path, &profile)
        .with_context(|| format!("write profile: {}", context.profile_path.display()))?;

    if context.global.json {
        println!(
            "{}",
            serde_json::json!({
                "profile": context.profile_path,
                "status": "written"
            })
        );
    } else {
        println!("profile imported: {}", context.profile_path.display());
    }

    Ok(())
}

fn show_profile(context: &AppContext) -> Result<()> {
    if context.global.json {
        println!("{}", serde_json::to_string_pretty(&context.profile)?);
        return Ok(());
    }

    println!("{}", context.resolved_profile_text()?);
    Ok(())
}

fn validate_profile(context: &AppContext) -> Result<()> {
    let _loaded = load_profile_from_path(&context.profile_path)?;

    for path in &context.global.env_files {
        let _parsed = parse_env_file(path)?;
    }

    let mut contract_errors = Vec::new();
    if let Some(contract) = context.contract.as_ref() {
        let mut contract_for_validation = contract.clone();
        if let Some(validation_mode) = context.global.validation_mode.as_ref() {
            contract_for_validation.policies.validation_mode = match validation_mode.trim() {
                "strict" => ValidationMode::Strict,
                "debug" => ValidationMode::Debug,
                other => {
                    return Err(anyhow!(
                        "invalid --validation-mode '{}'; expected strict or debug",
                        other
                    ));
                }
            };
        }

        if let Err(errors) = validate_contract(&contract_for_validation, &context.global.profile) {
            contract_errors = errors;
        }
    }

    if !contract_errors.is_empty() {
        let has_security_critical = contract_errors.iter().any(|error| error.security_critical);
        if has_security_critical || !context.global.force {
            if context.global.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "profile": context.global.profile,
                        "contract_path": context
                            .contract_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "valid": false,
                        "errors": contract_errors
                            .iter()
                            .map(|error| serde_json::json!({
                                "code": error.code,
                                "message": error.message,
                                "security_critical": error.security_critical,
                            }))
                            .collect::<Vec<_>>(),
                    })
                );
            }
            return Err(anyhow!(
                "contract validation failed for profile {}:\n{}",
                context.global.profile,
                contract_errors
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

        if context.global.json {
            println!(
                "{}",
                serde_json::json!({
                    "profile": context.global.profile,
                    "contract_path": context
                        .contract_path
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    "valid": true,
                    "warnings": contract_errors
                        .iter()
                        .map(|error| serde_json::json!({
                            "code": error.code,
                            "message": error.message,
                            "security_critical": error.security_critical,
                        }))
                        .collect::<Vec<_>>(),
                })
            );
        } else {
            println!(
                "profile validation passed with --force (non-security warnings):\n{}",
                contract_errors
                    .iter()
                    .map(|error| format!("- [{}] {}", error.code, error.message))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        return Ok(());
    }

    if context.global.json {
        println!(
            "{}",
            serde_json::json!({
                "profile": context.global.profile,
                "profile_path": context.profile_path,
                "contract_path": context
                    .contract_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                "valid": true
            })
        );
    } else {
        println!(
            "profile validation passed: {}",
            context.profile_path.display()
        );
    }

    Ok(())
}

fn merge_profile(target: &mut HookProfile, source: &HookProfile) {
    for (key, value) in &source.env {
        target.env.insert(key.clone(), value.clone());
    }

    if source.backends.serve_cmd.is_some() {
        target.backends.serve_cmd = source.backends.serve_cmd.clone();
    }
    if source.backends.smash_cmd.is_some() {
        target.backends.smash_cmd = source.backends.smash_cmd.clone();
    }

    if !source.capabilities.roles.is_empty() {
        target.capabilities.roles = source.capabilities.roles.clone();
    }
}
