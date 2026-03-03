use crate::config::Config;
use crate::dlq::DlqProducer;
use crate::forwarder::Forwarder;
use anyhow::{Context, Result};
use rdkafka::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message};
use relay_core::model::WebhookEnvelope;
use serde::Serialize;
use tracing::{Level, debug, error, info, warn};

const MAX_KAFKA_PAYLOAD_PREVIEW_CHARS: usize = 4_096;

pub struct KafkaConsumer {
    consumer: StreamConsumer,
    forwarder: Forwarder,
    dlq: DlqProducer,
}

impl KafkaConsumer {
    pub fn from_config(config: &Config, forwarder: Forwarder, dlq: DlqProducer) -> Result<Self> {
        let mut client_config = ClientConfig::new();
        client_config
            .set("bootstrap.servers", &config.kafka_brokers)
            .set("group.id", &config.kafka_group_id)
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "latest")
            .set("security.protocol", &config.kafka_security_protocol);

        if let Some(mechanism) = &config.kafka_sasl_mechanism {
            client_config.set("sasl.mechanism", mechanism);
        }
        if let Some(username) = &config.kafka_sasl_username {
            client_config.set("sasl.username", username);
        }
        if let Some(password) = &config.kafka_sasl_password {
            client_config.set("sasl.password", password);
        }

        let consumer = client_config
            .create::<StreamConsumer>()
            .context("create kafka stream consumer")?;

        let topic_refs = config
            .kafka_topics
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        consumer
            .subscribe(&topic_refs)
            .with_context(|| format!("subscribe to topics: {}", topic_refs.join(",")))?;
        info!(
            group_id = config.kafka_group_id.as_str(),
            topics = ?config.kafka_topics,
            "kafka consumer subscribed to topics"
        );

        Ok(Self {
            consumer,
            forwarder,
            dlq,
        })
    }

    pub async fn run(&self) -> Result<()> {
        info!("kafka-openclaw-hook started");

        loop {
            match self.consumer.recv().await {
                Ok(message) => {
                    if let Err(error) = self.process_message(message).await {
                        error!(error = %error, "failed to process kafka message");
                    }
                }
                Err(error) => {
                    warn!(error = %error, "kafka poll error");
                }
            }
        }
    }

    async fn process_message(&self, message: BorrowedMessage<'_>) -> Result<()> {
        let topic = message.topic().to_string();
        let partition = message.partition();
        let offset = message.offset();
        let key = message_key_preview(message.key());

        let payload_bytes = message.payload().context("kafka message missing payload")?;
        info!(
            topic = topic.as_str(),
            partition,
            offset,
            key = key.as_str(),
            payload_bytes = payload_bytes.len(),
            "received kafka message"
        );
        if tracing::enabled!(Level::DEBUG) {
            debug!(
                topic = topic.as_str(),
                partition,
                offset,
                raw_payload = %bytes_utf8_preview(payload_bytes, MAX_KAFKA_PAYLOAD_PREVIEW_CHARS),
                "kafka message payload preview"
            );
        }

        let envelope: WebhookEnvelope = serde_json::from_slice(payload_bytes)
            .context("deserialize webhook envelope from kafka")?;
        debug!(
            topic = topic.as_str(),
            partition,
            offset,
            event_id = envelope.id.as_str(),
            source = envelope.source.as_str(),
            event_type = envelope.event_type.as_str(),
            envelope_json = %to_json_string(&envelope),
            "deserialized webhook envelope from kafka"
        );

        if let Err(error) = self.forwarder.forward_with_retry(&envelope).await {
            warn!(
                topic = topic.as_str(),
                partition,
                offset,
                event_id = %envelope.id,
                source = %envelope.source,
                error = %error,
                "forwarding failed, publishing to dlq"
            );
            self.dlq
                .publish_failed(&envelope, &error.to_string())
                .await
                .context("publish dlq envelope")?;
        } else {
            info!(
                topic = topic.as_str(),
                partition,
                offset,
                event_id = %envelope.id,
                source = %envelope.source,
                event_type = %envelope.event_type,
                "forwarded to openclaw"
            );
        }

        self.consumer
            .commit_message(&message, CommitMode::Async)
            .context("commit kafka offset")?;
        debug!(
            topic = topic.as_str(),
            partition,
            offset,
            event_id = envelope.id.as_str(),
            "committed kafka offset"
        );

        Ok(())
    }
}

fn bytes_utf8_preview(bytes: &[u8], max_chars: usize) -> String {
    let raw = String::from_utf8_lossy(bytes);
    if raw.chars().count() <= max_chars {
        return raw.into_owned();
    }

    let preview_limit = max_chars.saturating_sub(3);
    let mut output = String::new();
    let mut char_count = 0usize;
    for character in raw.chars() {
        if char_count >= preview_limit {
            break;
        }
        output.push(character);
        char_count = char_count.saturating_add(1);
    }
    output.push_str("...");
    output
}

fn message_key_preview(key: Option<&[u8]>) -> String {
    match key {
        Some(bytes) => bytes_utf8_preview(bytes, MAX_KAFKA_PAYLOAD_PREVIEW_CHARS),
        None => "none".to_string(),
    }
}

fn to_json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|error| format!("{{\"serialization_error\":\"{}\"}}", error))
}
