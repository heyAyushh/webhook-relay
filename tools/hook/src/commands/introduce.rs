use crate::capabilities::detect_capabilities;
use crate::cli::IntroduceArgs;
use crate::config::{
    AppContext, HookProfile, load_profile_from_path, merge_env_files, write_profile,
};
use anyhow::{Context, Result};

pub async fn run(context: &AppContext, arguments: &IntroduceArgs) -> Result<()> {
    let mut profile = context.profile.clone();

    if let Some(source_profile_path) = &arguments.toml {
        let imported = load_profile_from_path(source_profile_path)?;
        merge_profile(&mut profile, &imported);
    }

    let imported_env = merge_env_files(&context.global.env_files)?;
    for (key, value) in imported_env {
        profile.env.insert(key, value);
    }

    if !arguments.dry_run {
        write_profile(&context.profile_path, &profile)
            .with_context(|| format!("write profile: {}", context.profile_path.display()))?;
        println!("profile written to {}", context.profile_path.display());
    } else {
        println!("dry run: profile not written");
    }

    let mut preview = context.clone();
    preview.profile = profile;

    let report = detect_capabilities(&preview);
    if context.global.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("capability matrix:");
    for role in report.roles {
        println!(
            "- role={} available={} reasons={}",
            serde_json::to_string(&role.role)?.trim_matches('"'),
            role.available,
            if role.reasons.is_empty() {
                "none".to_string()
            } else {
                role.reasons.join("; ")
            }
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
