use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use chrono::{SecondsFormat, Utc};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::{DlqEnvelope, WebhookEnvelope};
use std::time::Duration;

#[derive(Clone)]
pub struct DlqProducer {
    producer: FutureProducer,
    topic: String,
}

impl DlqProducer {
    pub fn from_config(config: &Config) -> Result<Self> {
        let producer = ClientConfig::new()
            .set("bootstrap.servers", &config.kafka_brokers)
            .set("security.protocol", "ssl")
            .set("ssl.certificate.location", &config.kafka_tls_cert)
            .set("ssl.key.location", &config.kafka_tls_key)
            .set("ssl.ca.location", &config.kafka_tls_ca)
            .set("message.timeout.ms", "5000")
            .set("queue.buffering.max.ms", "5")
            .create::<FutureProducer>()
            .context("create dlq producer")?;

        Ok(Self {
            producer,
            topic: config.dlq_topic.clone(),
        })
    }

    pub async fn publish_failed(
        &self,
        envelope: &WebhookEnvelope,
        error_message: &str,
    ) -> Result<()> {
        let dlq_payload = DlqEnvelope {
            failed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            error: error_message.to_string(),
            envelope: envelope.clone(),
        };

        let payload = serde_json::to_string(&dlq_payload).context("serialize dlq envelope")?;
        let key = envelope.id.as_str();

        self.producer
            .send(
                FutureRecord::to(&self.topic).key(key).payload(&payload),
                Timeout::After(Duration::from_secs(5)),
            )
            .await
            .map_err(|(error, _)| anyhow!("publish dlq message failed: {error}"))?;

        Ok(())
    }
}
