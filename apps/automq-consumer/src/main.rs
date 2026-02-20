mod config;
mod consumer;
mod dlq;
mod forwarder;

use anyhow::{Context, Result};
use config::Config;
use consumer::KafkaConsumer;
use dlq::DlqProducer;
use forwarder::Forwarder;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config::from_env().context("load consumer config")?;
    let forwarder = Forwarder::new(config.clone()).context("initialize forwarder")?;
    let dlq = DlqProducer::from_config(&config).context("initialize dlq producer")?;
    let consumer =
        KafkaConsumer::from_config(&config, forwarder, dlq).context("initialize consumer")?;

    consumer.run().await
}
