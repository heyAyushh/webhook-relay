use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::types::RDKafkaErrorCode;
use rdkafka::util::Timeout;
use relay_core::model::{Source, WebhookEnvelope};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct PublishJob {
    pub topic: String,
    pub envelope: WebhookEnvelope,
}

#[derive(Clone)]
pub struct KafkaPublisher {
    producer: FutureProducer,
    max_retries: u32,
    backoff_base_ms: u64,
    backoff_max_ms: u64,
}

impl KafkaPublisher {
    pub fn from_config(config: &Config) -> Result<Self> {
        let producer = base_client_config(config)
            .set("message.timeout.ms", "5000")
            .set("queue.buffering.max.ms", "5")
            .create::<FutureProducer>()
            .context("create kafka future producer")?;

        Ok(Self {
            producer,
            max_retries: config.publish_max_retries,
            backoff_base_ms: config.publish_backoff_base_ms,
            backoff_max_ms: config.publish_backoff_max_ms,
        })
    }

    pub async fn publish(&self, job: &PublishJob) -> Result<()> {
        let payload = serde_json::to_string(&job.envelope).context("serialize webhook envelope")?;
        let key = job.envelope.id.as_str();

        let mut attempt = 0u32;
        loop {
            let record = FutureRecord::to(&job.topic).key(key).payload(&payload);
            match self
                .producer
                .send(record, Timeout::After(Duration::from_secs(5)))
                .await
            {
                Ok(_) => return Ok(()),
                Err((error, _message)) => {
                    attempt = attempt.saturating_add(1);
                    if attempt >= self.max_retries {
                        return Err(anyhow!(
                            "kafka publish failed after {attempt} attempts: {error}"
                        ));
                    }

                    let backoff = retry_backoff_ms(
                        self.backoff_base_ms,
                        self.backoff_max_ms,
                        attempt.saturating_sub(1),
                    );
                    warn!(
                        topic = %job.topic,
                        event_id = %job.envelope.id,
                        attempt,
                        backoff_ms = backoff,
                        error = %error,
                        "kafka publish failed; retrying"
                    );
                    sleep(Duration::from_millis(backoff)).await;
                }
            }
        }
    }
}

pub async fn ensure_required_topics(config: &Config) -> Result<()> {
    if !config.kafka_auto_create_topics {
        info!("kafka topic auto-create disabled");
        return Ok(());
    }

    let admin_client: AdminClient<DefaultClientContext> = base_client_config(config)
        .create()
        .context("create kafka admin client")?;

    let topics = vec![
        NewTopic::new(
            Source::Github.topic_name(),
            config.kafka_topic_partitions,
            TopicReplication::Fixed(config.kafka_topic_replication_factor),
        ),
        NewTopic::new(
            Source::Linear.topic_name(),
            config.kafka_topic_partitions,
            TopicReplication::Fixed(config.kafka_topic_replication_factor),
        ),
        NewTopic::new(
            &config.kafka_dlq_topic,
            config.kafka_topic_partitions,
            TopicReplication::Fixed(config.kafka_topic_replication_factor),
        ),
    ];

    let results = admin_client
        .create_topics(&topics, &AdminOptions::new())
        .await
        .context("create kafka topics")?;

    for result in results {
        match result {
            Ok(topic_name) => {
                info!(topic = %topic_name, "kafka topic ready");
            }
            Err((topic_name, RDKafkaErrorCode::TopicAlreadyExists)) => {
                info!(topic = %topic_name, "kafka topic already exists");
            }
            Err((topic_name, error_code)) => {
                return Err(anyhow!(
                    "failed to create topic {}: {}",
                    topic_name,
                    error_code
                ));
            }
        }
    }

    Ok(())
}

pub async fn run_publish_worker(mut rx: mpsc::Receiver<PublishJob>, publisher: KafkaPublisher) {
    while let Some(job) = rx.recv().await {
        if let Err(error) = publisher.publish(&job).await {
            error!(
                topic = %job.topic,
                event_id = %job.envelope.id,
                error = %error,
                "failed to publish envelope to kafka"
            );
        }
    }
}

pub fn retry_backoff_ms(base_ms: u64, max_ms: u64, attempt_index: u32) -> u64 {
    let exponent = attempt_index.min(31);
    let scaled = base_ms.saturating_mul(1u64 << exponent);
    scaled.min(max_ms)
}

fn base_client_config(config: &Config) -> ClientConfig {
    let mut client_config = ClientConfig::new();
    client_config
        .set("bootstrap.servers", &config.kafka_brokers)
        .set("security.protocol", "ssl")
        .set("ssl.certificate.location", &config.kafka_tls_cert)
        .set("ssl.key.location", &config.kafka_tls_key)
        .set("ssl.ca.location", &config.kafka_tls_ca);
    client_config
}

#[cfg(test)]
mod tests {
    use super::retry_backoff_ms;

    #[test]
    fn backoff_caps_at_max() {
        assert_eq!(retry_backoff_ms(100, 1000, 0), 100);
        assert_eq!(retry_backoff_ms(100, 1000, 1), 200);
        assert_eq!(retry_backoff_ms(100, 1000, 2), 400);
        assert_eq!(retry_backoff_ms(100, 1000, 3), 800);
        assert_eq!(retry_backoff_ms(100, 1000, 4), 1000);
        assert_eq!(retry_backoff_ms(100, 1000, 5), 1000);
    }
}
