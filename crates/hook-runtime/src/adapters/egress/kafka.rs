use crate::smash::config::Config;
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::WebhookEnvelope;
use std::time::Duration;

const DEFAULT_OUTPUT_TIMEOUT_SECONDS: u64 = 5;

#[derive(Clone)]
pub struct KafkaOutputAdapter {
    topic: String,
    key_mode: String,
    producer: FutureProducer,
}

impl KafkaOutputAdapter {
    pub fn from_config(config: &Config, topic: String, key_mode: String) -> Result<Self> {
        let producer = build_future_producer(config).context("create kafka output producer")?;
        Ok(Self {
            topic,
            key_mode,
            producer,
        })
    }

    pub async fn publish(&self, envelope: &WebhookEnvelope) -> Result<()> {
        let payload =
            serde_json::to_string(envelope).context("serialize envelope for kafka_output")?;
        let key = match self.key_mode.as_str() {
            "event_id" => Some(envelope.id.clone()),
            "source" => Some(envelope.source.clone()),
            "none" => None,
            other => return Err(anyhow!("unsupported kafka_output key_mode '{}'", other)),
        };

        let mut record = FutureRecord::to(&self.topic).payload(&payload);
        if let Some(key) = key.as_ref() {
            record = record.key(key);
        }

        self.producer
            .send(
                record,
                Timeout::After(Duration::from_secs(DEFAULT_OUTPUT_TIMEOUT_SECONDS)),
            )
            .await
            .map_err(|(error, _)| anyhow!("kafka_output send failed: {}", error))?;
        Ok(())
    }
}

fn build_future_producer(config: &Config) -> Result<FutureProducer> {
    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", &config.kafka_brokers)
        .set("security.protocol", &config.kafka_security_protocol)
        .set("message.timeout.ms", "5000")
        .set("queue.buffering.max.ms", "5");

    if let Some(mechanism) = &config.kafka_sasl_mechanism {
        client_config.set("sasl.mechanism", mechanism);
    }
    if let Some(username) = &config.kafka_sasl_username {
        client_config.set("sasl.username", username);
    }
    if let Some(password) = &config.kafka_sasl_password {
        client_config.set("sasl.password", password);
    }

    client_config
        .create::<FutureProducer>()
        .context("create future producer")
}
