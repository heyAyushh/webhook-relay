use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "hook")]
#[command(about = "Utility CLI for serve/relay/smash and operations")]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    #[arg(long, default_value = "default-openclaw")]
    pub profile: String,
    #[arg(long)]
    pub app: Option<String>,
    #[arg(long)]
    pub contract: Option<PathBuf>,
    #[arg(long = "env-file")]
    pub env_files: Vec<PathBuf>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub validation_mode: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum HookCommand {
    Serve(ServeArgs),
    Relay(RelayArgs),
    Smash(SmashArgs),
    Test(TestArgs),
    Replay(ReplayArgs),
    Debug(DebugArgs),
    Introduce(IntroduceArgs),
    Config(ConfigArgs),
    Infra(InfraArgs),
    Logs(LogsArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub bind: Option<String>,
    #[arg(long)]
    pub enabled_sources: Option<String>,
    #[arg(long)]
    pub brokers: Option<String>,
    #[arg(long)]
    pub source_topic_prefix: Option<String>,
    #[arg(long)]
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SmashArgs {
    #[arg(long)]
    pub topics: Option<String>,
    #[arg(long)]
    pub webhook_url: Option<String>,
    #[arg(long)]
    pub webhook_token: Option<String>,
    #[arg(long)]
    pub group_id: Option<String>,
    #[arg(long)]
    pub brokers: Option<String>,
    #[arg(long)]
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayMode {
    Envelope,
    Raw,
}

#[derive(Debug, Clone, Args)]
pub struct RelayArgs {
    #[arg(long)]
    pub topics: Option<String>,
    #[arg(long)]
    pub output_topic: Option<String>,
    #[arg(long)]
    pub group_id: Option<String>,
    #[arg(long)]
    pub brokers: Option<String>,
    #[arg(long)]
    pub instance_id: Option<String>,
    #[arg(long, value_enum, default_value = "envelope")]
    pub mode: RelayMode,
    #[arg(long)]
    pub max_retries: Option<u32>,
    #[arg(long)]
    pub backoff_base_ms: Option<u64>,
    #[arg(long)]
    pub backoff_max_ms: Option<u64>,
}

#[derive(Debug, Clone, Args)]
pub struct TestArgs {
    #[command(subcommand)]
    pub command: TestCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum TestCommand {
    Env,
    Smoke(SmokeArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SmokeArgs {
    #[command(subcommand)]
    pub command: SmokeCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum SmokeCommand {
    Serve {
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    Relay,
    Smash,
}

#[derive(Debug, Clone, Args)]
pub struct ReplayArgs {
    #[command(subcommand)]
    pub command: ReplayCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ReplayCommand {
    Webhook(ReplayWebhookArgs),
    Kafka(ReplayKafkaArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ReplayWebhookArgs {
    #[arg(long)]
    pub url: String,
    #[arg(long)]
    pub file: PathBuf,
    #[arg(long)]
    pub source: Option<String>,
    #[arg(long = "header")]
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ReplayKafkaArgs {
    #[arg(long)]
    pub topic: String,
    #[arg(long)]
    pub file: PathBuf,
    #[arg(long)]
    pub brokers: Option<String>,
    #[arg(long)]
    pub key: Option<String>,
    #[arg(long, value_enum, default_value = "raw")]
    pub mode: RelayMode,
}

#[derive(Debug, Clone, Args)]
pub struct DebugArgs {
    #[command(subcommand)]
    pub command: DebugCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum DebugCommand {
    Capabilities,
    Env {
        #[arg(long)]
        no_redact: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct IntroduceArgs {
    #[arg(long)]
    pub toml: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    Import(ConfigImportArgs),
    Show,
    Validate,
}

#[derive(Debug, Clone, Args)]
pub struct ConfigImportArgs {
    #[arg(long = "env-file")]
    pub env_files: Vec<PathBuf>,
    #[arg(long)]
    pub toml: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct InfraArgs {
    #[command(subcommand)]
    pub command: InfraCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraCommand {
    Docker(InfraDockerArgs),
    Firecracker(InfraFirecrackerArgs),
    Broker(InfraBrokerArgs),
    Systemd(InfraSystemdArgs),
    Certs(InfraCertsArgs),
}

#[derive(Debug, Clone, Args)]
pub struct InfraDockerArgs {
    #[command(subcommand)]
    pub command: InfraDockerCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraDockerCommand {
    Up {
        #[arg(long)]
        dev: bool,
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    Down {
        #[arg(long)]
        dev: bool,
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    Logs {
        #[arg(long)]
        dev: bool,
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
}

#[derive(Debug, Clone, Args)]
pub struct InfraFirecrackerArgs {
    #[command(subcommand)]
    pub command: InfraFirecrackerCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraFirecrackerCommand {
    Run {
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    NetworkUp {
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    NetworkDown {
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
    BuildRootfs {
        #[arg(last = true)]
        passthrough: Vec<String>,
    },
}

#[derive(Debug, Clone, Args)]
pub struct InfraBrokerArgs {
    #[command(subcommand)]
    pub command: InfraBrokerCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraBrokerCommand {
    List,
    Show,
}

#[derive(Debug, Clone, Args)]
pub struct InfraSystemdArgs {
    #[command(subcommand)]
    pub command: InfraSystemdCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraSystemdCommand {
    Status {
        unit: String,
    },
    Logs {
        unit: String,
        #[arg(long, default_value_t = 200)]
        lines: usize,
    },
    Restart {
        unit: String,
    },
}

#[derive(Debug, Clone, Args)]
pub struct InfraCertsArgs {
    #[command(subcommand)]
    pub command: InfraCertsCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum InfraCertsCommand {
    Gen {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        ca: bool,
        #[arg(long)]
        relay: bool,
        #[arg(long)]
        consumer: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct LogsArgs {
    #[command(subcommand)]
    pub command: LogsCommand,
}

#[derive(Debug, Clone, Subcommand)]
pub enum LogsCommand {
    Collect(LogsCollectArgs),
    Sources,
    Tail(LogsTailArgs),
}

#[derive(Debug, Clone, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsScope {
    Auto,
    Full,
    Runtime,
    System,
}

#[derive(Debug, Clone, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogsFormat {
    Bundle,
    Stream,
    Both,
}

#[derive(Debug, Clone, Args)]
pub struct LogsCollectArgs {
    #[arg(long, value_enum, default_value = "auto")]
    pub scope: LogsScope,
    #[arg(long, value_enum, default_value = "both")]
    pub format: LogsFormat,
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub until: Option<String>,
    #[arg(long, default_value_t = 300)]
    pub lines: usize,
    #[arg(long)]
    pub output: Option<PathBuf>,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub redact: bool,
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_redact: bool,
}

#[derive(Debug, Clone, Args)]
pub struct LogsTailArgs {
    #[arg(long, value_enum, default_value = "auto")]
    pub scope: LogsScope,
    #[arg(long, default_value_t = 100)]
    pub lines: usize,
    #[arg(long)]
    pub follow: bool,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub redact: bool,
    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_redact: bool,
}
