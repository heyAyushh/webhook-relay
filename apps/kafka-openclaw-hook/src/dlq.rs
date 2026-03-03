use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use chrono::{SecondsFormat, Utc};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::{DlqEnvelope, WebhookEnvelope};
use std::time::Duration;
use tracing::{debug, info};

#[derive(Clone)]
pub struct DlqProducer {
    producer: FutureProducer,
    topic: String,
}

impl DlqProducer {
    pub fn from_config(config: &Config) -> Result<Self> {
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

        let producer = client_config
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
        debug!(
            topic = self.topic.as_str(),
            event_id = envelope.id.as_str(),
            source = envelope.source.as_str(),
            event_type = envelope.event_type.as_str(),
            dlq_payload = payload.as_str(),
            "publishing failed envelope to dlq"
        );

        self.producer
            .send(
                FutureRecord::to(&self.topic).key(key).payload(&payload),
                Timeout::After(Duration::from_secs(5)),
            )
            .await
            .map_err(|(error, _)| anyhow!("publish dlq message failed: {error}"))?;
        info!(
            topic = self.topic.as_str(),
            event_id = envelope.id.as_str(),
            source = envelope.source.as_str(),
            event_type = envelope.event_type.as_str(),
            "published failed envelope to dlq"
        );

        Ok(())
    }
}
