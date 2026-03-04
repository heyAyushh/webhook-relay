use crate::cli::{
    InfraArgs, InfraBrokerArgs, InfraBrokerCommand, InfraCertsArgs, InfraCertsCommand,
    InfraCommand, InfraDockerArgs, InfraDockerCommand, InfraFirecrackerArgs,
    InfraFirecrackerCommand, InfraSystemdArgs, InfraSystemdCommand,
};
use crate::config::AppContext;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub async fn run(context: &AppContext, arguments: &InfraArgs) -> Result<()> {
    match &arguments.command {
        InfraCommand::Docker(details) => run_docker(context, details),
        InfraCommand::Firecracker(details) => run_firecracker(context, details),
        InfraCommand::Broker(details) => run_broker(context, details),
        InfraCommand::Systemd(details) => run_systemd(details),
        InfraCommand::Certs(details) => run_certs(context, details),
    }
}

fn run_docker(context: &AppContext, arguments: &InfraDockerArgs) -> Result<()> {
    let repo_root = context
        .repo_root
        .as_ref()
        .ok_or_else(|| anyhow!("repo root not detected; docker wrapper unavailable"))?;

    let (dev, action, passthrough) = match &arguments.command {
        InfraDockerCommand::Up { dev, passthrough } => (*dev, "up", passthrough.clone()),
        InfraDockerCommand::Down { dev, passthrough } => (*dev, "down", passthrough.clone()),
        InfraDockerCommand::Logs { dev, passthrough } => (*dev, "logs", passthrough.clone()),
    };

    let mut command = Command::new("docker");
    command
        .arg("compose")
        .arg("-f")
        .arg(repo_root.join("docker-compose.yml"));

    if dev {
        command
            .arg("-f")
            .arg(repo_root.join("docker-compose.dev.yml"));
    }

    command.arg(action);
    if action == "up" {
        command.arg("--build");
    }
    command.args(passthrough);

    run_command(command, Some(repo_root.clone()))
}

fn run_firecracker(context: &AppContext, arguments: &InfraFirecrackerArgs) -> Result<()> {
    let repo_root = context
        .repo_root
        .as_ref()
        .ok_or_else(|| anyhow!("repo root not detected; firecracker wrapper unavailable"))?;

    let (script_path, passthrough) = match &arguments.command {
        InfraFirecrackerCommand::Run { passthrough } => (
            repo_root.join("scripts/_run-firecracker.sh"),
            passthrough.clone(),
        ),
        InfraFirecrackerCommand::NetworkUp { passthrough } => (
            repo_root.join("scripts/_setup-firecracker-bridge-network.sh"),
            passthrough.clone(),
        ),
        InfraFirecrackerCommand::NetworkDown { passthrough } => (
            repo_root.join("scripts/_teardown-firecracker-bridge-network.sh"),
            passthrough.clone(),
        ),
        InfraFirecrackerCommand::BuildRootfs { passthrough } => (
            repo_root.join("scripts/_build-firecracker-rootfs.sh"),
            passthrough.clone(),
        ),
    };

    if !script_path.exists() {
        return Err(anyhow!("script missing: {}", script_path.display()));
    }

    let mut command = Command::new("bash");
    command.arg(script_path).args(passthrough);

    run_command(command, Some(repo_root.clone()))
}

fn run_broker(context: &AppContext, arguments: &InfraBrokerArgs) -> Result<()> {
    let repo_root = context
        .repo_root
        .as_ref()
        .ok_or_else(|| anyhow!("repo root not detected; broker wrapper unavailable"))?;
    let script = repo_root.join("firecracker/runtime/broker_inventory.sh");
    if !script.exists() {
        return Err(anyhow!(
            "broker inventory script missing: {}",
            script.display()
        ));
    }

    let bash_expression = match &arguments.command {
        InfraBrokerCommand::List => {
            format!("source '{}' && inventory_rows", script.display())
        }
        InfraBrokerCommand::Show => {
            format!("source '{}' && inventory_primary_row", script.display())
        }
    };

    let mut command = Command::new("bash");
    command.arg("-lc").arg(bash_expression);

    run_command(command, Some(repo_root.clone()))
}

fn run_systemd(arguments: &InfraSystemdArgs) -> Result<()> {
    let command = match &arguments.command {
        InfraSystemdCommand::Status { unit } => {
            let mut command = Command::new("systemctl");
            command.arg("status").arg("--no-pager").arg(unit);
            command
        }
        InfraSystemdCommand::Logs { unit, lines } => {
            let mut command = Command::new("journalctl");
            command
                .arg("-u")
                .arg(unit)
                .arg("--no-pager")
                .arg("-n")
                .arg(lines.to_string());
            command
        }
        InfraSystemdCommand::Restart { unit } => {
            let mut command = Command::new("systemctl");
            command.arg("restart").arg(unit);
            command
        }
    };

    run_command(command, None)
}

fn run_certs(context: &AppContext, arguments: &InfraCertsArgs) -> Result<()> {
    match &arguments.command {
        InfraCertsCommand::Gen {
            dir,
            ca,
            relay,
            consumer,
        } => generate_certs(context, dir.as_ref(), *ca, *relay, *consumer),
    }
}

fn generate_certs(
    context: &AppContext,
    output_dir_flag: Option<&PathBuf>,
    generate_ca_flag: bool,
    generate_relay_flag: bool,
    generate_consumer_flag: bool,
) -> Result<()> {
    let output_dir = resolve_certs_output_dir(context, output_dir_flag);
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create cert directory: {}", output_dir.display()))?;

    let all_default = !generate_ca_flag && !generate_relay_flag && !generate_consumer_flag;
    let generate_ca = generate_ca_flag || all_default;
    let generate_relay = generate_relay_flag || all_default;
    let generate_consumer = generate_consumer_flag || all_default;

    ensure_openssl_available()?;

    let ca_key_path = output_dir.join("ca.key");
    let ca_crt_path = output_dir.join("ca.crt");

    if generate_ca {
        run_openssl(&["genrsa", "-out", path_arg(&ca_key_path), CERT_KEY_BITS])?;
        run_openssl(&[
            "req",
            "-x509",
            "-new",
            "-nodes",
            "-key",
            path_arg(&ca_key_path),
            "-sha256",
            "-days",
            CERT_VALID_DAYS,
            "-subj",
            CERT_CA_SUBJECT,
            "-out",
            path_arg(&ca_crt_path),
        ])?;
    } else if generate_relay || generate_consumer {
        if !ca_key_path.exists() || !ca_crt_path.exists() {
            return Err(anyhow!(
                "CA files are required to generate client certs; missing {} or {}",
                ca_key_path.display(),
                ca_crt_path.display()
            ));
        }
    }

    if generate_relay {
        write_client_cert("relay", &output_dir, &ca_key_path, &ca_crt_path)?;
    }
    if generate_consumer {
        write_client_cert("consumer", &output_dir, &ca_key_path, &ca_crt_path)?;
    }

    println!("certificates generated in: {}", output_dir.display());
    if generate_ca {
        println!("- CA: {}", ca_crt_path.display());
    }
    if generate_relay {
        println!(
            "- relay: {} + {}",
            output_dir.join("relay.crt").display(),
            output_dir.join("relay.key").display()
        );
    }
    if generate_consumer {
        println!(
            "- consumer: {} + {}",
            output_dir.join("consumer.crt").display(),
            output_dir.join("consumer.key").display()
        );
    }

    Ok(())
}

fn resolve_certs_output_dir(context: &AppContext, output_dir_flag: Option<&PathBuf>) -> PathBuf {
    match output_dir_flag {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => context
            .repo_root
            .as_ref()
            .map(|repo_root| repo_root.join(path))
            .unwrap_or_else(|| path.clone()),
        None => context
            .repo_root
            .as_ref()
            .map(|repo_root| repo_root.join("certs"))
            .unwrap_or_else(|| PathBuf::from("certs")),
    }
}

fn ensure_openssl_available() -> Result<()> {
    let mut command = Command::new("openssl");
    command.arg("version");
    run_command(command, None)
}

fn write_client_cert(
    name: &str,
    output_dir: &PathBuf,
    ca_key_path: &PathBuf,
    ca_crt_path: &PathBuf,
) -> Result<()> {
    let key_file = output_dir.join(format!("{name}.key"));
    let csr_file = output_dir.join(format!("{name}.csr"));
    let crt_file = output_dir.join(format!("{name}.crt"));
    let ext_file = output_dir.join(format!("{name}.ext"));
    let subject = format!("/CN={name}");

    run_openssl(&["genrsa", "-out", path_arg(&key_file), CERT_KEY_BITS])?;
    run_openssl(&[
        "req",
        "-new",
        "-key",
        path_arg(&key_file),
        "-subj",
        subject.as_str(),
        "-out",
        path_arg(&csr_file),
    ])?;

    fs::write(
        &ext_file,
        "keyUsage = critical,digitalSignature,keyEncipherment\nextendedKeyUsage = clientAuth\nsubjectAltName = DNS:localhost\n",
    )
    .with_context(|| format!("write cert extension file: {}", ext_file.display()))?;

    run_openssl(&[
        "x509",
        "-req",
        "-in",
        path_arg(&csr_file),
        "-CA",
        path_arg(ca_crt_path),
        "-CAkey",
        path_arg(ca_key_path),
        "-CAcreateserial",
        "-out",
        path_arg(&crt_file),
        "-days",
        CERT_VALID_DAYS,
        "-sha256",
        "-extfile",
        path_arg(&ext_file),
    ])?;

    if csr_file.exists() {
        fs::remove_file(&csr_file)
            .with_context(|| format!("remove temporary csr file: {}", csr_file.display()))?;
    }
    if ext_file.exists() {
        fs::remove_file(&ext_file)
            .with_context(|| format!("remove temporary ext file: {}", ext_file.display()))?;
    }

    Ok(())
}

fn run_openssl(arguments: &[&str]) -> Result<()> {
    let mut command = Command::new("openssl");
    command.args(arguments);
    run_command(command, None)
}

fn path_arg(path: &PathBuf) -> &str {
    path.to_str().unwrap_or("")
}

const CERT_KEY_BITS: &str = "4096";
const CERT_VALID_DAYS: &str = "825";
const CERT_CA_SUBJECT: &str = "/CN=OpenClaw-AutoMQ-CA";

fn run_command(mut command: Command, working_directory: Option<PathBuf>) -> Result<()> {
    command
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdin(Stdio::inherit());

    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }

    let status = command
        .status()
        .with_context(|| format!("run command: {:?}", command))?;
    if !status.success() {
        return Err(anyhow!("command exited with status {}", status));
    }

    Ok(())
}
