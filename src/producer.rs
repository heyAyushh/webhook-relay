use crate::config::Config;
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::WebhookEnvelope;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::{error, warn};

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
        let producer = ClientConfig::new()
            .set("bootstrap.servers", &config.kafka_brokers)
            .set("security.protocol", "ssl")
            .set("ssl.certificate.location", &config.kafka_tls_cert)
            .set("ssl.key.location", &config.kafka_tls_key)
            .set("ssl.ca.location", &config.kafka_tls_ca)
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
