pub(crate) mod config;
mod consumer;
mod dlq;

pub use config::Config;

use anyhow::{Context, Result};
use consumer::KafkaConsumer;
use dlq::DlqProducer;

pub async fn run_from_env() -> Result<()> {
    let config = Config::from_env().context("load smash config")?;
    let dlq = DlqProducer::from_config(&config).context("initialize dlq producer")?;
    let consumer = KafkaConsumer::from_config(&config, dlq)
        .await
        .context("initialize smash consumer")?;

    consumer.run().await
}
