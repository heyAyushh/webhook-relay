mod capabilities;
mod cli;
mod commands;
mod config;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, HookCommand};
use config::AppContext;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let cli = Cli::parse();
    let load_contract_config = !matches!(cli.command, HookCommand::Relay(_));
    let context = AppContext::load_with_contract(cli.global.clone(), load_contract_config)?;

    match &cli.command {
        HookCommand::Serve(arguments) => commands::serve::run(&context, arguments).await,
        HookCommand::Relay(arguments) => commands::relay::run(&context, arguments).await,
        HookCommand::Smash(arguments) => commands::smash::run(&context, arguments).await,
        HookCommand::Test(arguments) => commands::test::run(&context, arguments).await,
        HookCommand::Replay(arguments) => commands::replay::run(&context, arguments).await,
        HookCommand::Debug(arguments) => commands::debug::run(&context, arguments).await,
        HookCommand::Introduce(arguments) => commands::introduce::run(&context, arguments).await,
        HookCommand::Config(arguments) => commands::config::run(&context, arguments).await,
        HookCommand::Infra(arguments) => commands::infra::run(&context, arguments).await,
        HookCommand::Logs(arguments) => commands::logs::run(&context, arguments).await,
    }
}
