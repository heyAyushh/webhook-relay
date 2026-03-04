use crate::cli::{RelayArgs, RelayMode};
use crate::config::AppContext;
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::WebhookEnvelope;
use std::time::Duration;
use tokio::time::sleep;

const DEFAULT_RELAY_GROUP_ID: &str = "hook-relay";
const DEFAULT_RELAY_MAX_RETRIES: u32 = 5;
const DEFAULT_RELAY_BACKOFF_BASE_MS: u64 = 200;
const DEFAULT_RELAY_BACKOFF_MAX_MS: u64 = 5_000;
const DEFAULT_KAFKA_MESSAGE_TIMEOUT_SECONDS: u64 = 5;

#[derive(Debug, Clone)]
struct RelayRuntimeConfig {
    brokers: String,
    topics: Vec<String>,
    output_topic: String,
    group_id: String,
    mode: RelayMode,
    max_retries: u32,
    backoff_base_ms: u64,
    backoff_max_ms: u64,
    security_protocol: String,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tls_ca: Option<String>,
    sasl_mechanism: Option<String>,
    sasl_username: Option<String>,
    sasl_password: Option<String>,
    instance_id: Option<String>,
}

enum ProcessOutcome {
    Committed,
    DeferredNoCommit,
}

pub async fn run(context: &AppContext, arguments: &RelayArgs) -> Result<()> {
    let runtime = load_runtime_config(context, arguments)?;
    ensure_preconditions(context, &runtime)?;

    let consumer = build_consumer(&runtime)?;
    let producer = build_producer(&runtime)?;

    let topic_refs = runtime
        .topics
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    consumer
        .subscribe(&topic_refs)
        .with_context(|| format!("subscribe to topics: {}", topic_refs.join(",")))?;

    eprintln!(
        "hook relay started mode={:?} group_id={} topics={} output_topic={}",
        runtime.mode,
        runtime.group_id,
        runtime.topics.join(","),
        runtime.output_topic
    );

    loop {
        match consumer.recv().await {
            Ok(message) => match process_message(&consumer, &producer, &runtime, message).await {
                Ok(ProcessOutcome::Committed) => {}
                Ok(ProcessOutcome::DeferredNoCommit) => {}
                Err(error) => {
                    eprintln!("hook relay processing error: {error}");
                }
            },
            Err(error) => {
                eprintln!("hook relay poll error: {error}");
            }
        }
    }
}

fn load_runtime_config(context: &AppContext, arguments: &RelayArgs) -> Result<RelayRuntimeConfig> {
    let brokers = context
        .resolve_value(arguments.brokers.as_deref(), "KAFKA_BROKERS")
        .ok_or_else(|| anyhow!("missing KAFKA_BROKERS"))?;

    let topics_raw = context
        .resolve_value(arguments.topics.as_deref(), "KAFKA_TOPICS")
        .ok_or_else(|| anyhow!("missing KAFKA_TOPICS or --topics"))?;
    let topics = parse_topics(&topics_raw);
    if topics.is_empty() {
        return Err(anyhow!("resolved KAFKA_TOPICS is empty"));
    }

    let output_topic = context
        .resolve_value(arguments.output_topic.as_deref(), "HOOK_RELAY_OUTPUT_TOPIC")
        .ok_or_else(|| anyhow!("missing HOOK_RELAY_OUTPUT_TOPIC or --output-topic"))?;

    let group_id = context
        .resolve_value(arguments.group_id.as_deref(), "KAFKA_GROUP_ID")
        .unwrap_or_else(|| DEFAULT_RELAY_GROUP_ID.to_string());

    let security_protocol = context
        .resolve_value(None, "KAFKA_SECURITY_PROTOCOL")
        .unwrap_or_else(|| "plaintext".to_string())
        .to_ascii_lowercase();

    let max_retries = arguments.max_retries.unwrap_or_else(|| {
        context
            .resolve_value(None, "HOOK_RELAY_MAX_RETRIES")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(DEFAULT_RELAY_MAX_RETRIES)
    });

    let backoff_base_ms = arguments.backoff_base_ms.unwrap_or_else(|| {
        context
            .resolve_value(None, "HOOK_RELAY_BACKOFF_BASE_MS")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RELAY_BACKOFF_BASE_MS)
    });

    let backoff_max_ms = arguments.backoff_max_ms.unwrap_or_else(|| {
        context
            .resolve_value(None, "HOOK_RELAY_BACKOFF_MAX_MS")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_RELAY_BACKOFF_MAX_MS)
    });

    Ok(RelayRuntimeConfig {
        brokers,
        topics,
        output_topic,
        group_id,
        mode: arguments.mode.clone(),
        max_retries,
        backoff_base_ms,
        backoff_max_ms,
        security_protocol,
        tls_cert: context.resolve_value(None, "KAFKA_TLS_CERT"),
        tls_key: context.resolve_value(None, "KAFKA_TLS_KEY"),
        tls_ca: context.resolve_value(None, "KAFKA_TLS_CA"),
        sasl_mechanism: context.resolve_value(None, "KAFKA_SASL_MECHANISM"),
        sasl_username: context.resolve_value(None, "KAFKA_SASL_USERNAME"),
        sasl_password: context.resolve_value(None, "KAFKA_SASL_PASSWORD"),
        instance_id: context.resolve_value(arguments.instance_id.as_deref(), "HOOK_INSTANCE_ID"),
    })
}

fn ensure_preconditions(context: &AppContext, runtime: &RelayRuntimeConfig) -> Result<()> {
    let mut reasons = Vec::new();

    if runtime.security_protocol == "ssl" {
        if runtime.tls_cert.is_none() {
            reasons.push("missing KAFKA_TLS_CERT for ssl mode".to_string());
        }
        if runtime.tls_key.is_none() {
            reasons.push("missing KAFKA_TLS_KEY for ssl mode".to_string());
        }
        if runtime.tls_ca.is_none() {
            reasons.push("missing KAFKA_TLS_CA for ssl mode".to_string());
        }
    }

    if runtime.max_retries == 0 {
        reasons.push("max retries must be >= 1".to_string());
    }

    if !reasons.is_empty() && !context.global.force {
        return Err(anyhow!(
            "relay unavailable: {}. use --force to bypass",
            reasons.join("; ")
        ));
    }

    Ok(())
}

fn build_consumer(runtime: &RelayRuntimeConfig) -> Result<StreamConsumer> {
    let mut config = ClientConfig::new();
    config
        .set("bootstrap.servers", &runtime.brokers)
        .set("group.id", &runtime.group_id)
        .set("enable.auto.commit", "false")
        .set("auto.offset.reset", "latest");

    apply_security_settings(&mut config, runtime);

    if let Some(instance_id) = &runtime.instance_id {
        config.set("client.id", instance_id);
    }

    config
        .create::<StreamConsumer>()
        .context("create relay stream consumer")
}

fn build_producer(runtime: &RelayRuntimeConfig) -> Result<FutureProducer> {
    let mut config = ClientConfig::new();
    config.set("bootstrap.servers", &runtime.brokers).set(
        "message.timeout.ms",
        format!("{}", DEFAULT_KAFKA_MESSAGE_TIMEOUT_SECONDS * 1000),
    );

    apply_security_settings(&mut config, runtime);

    if let Some(instance_id) = &runtime.instance_id {
        config.set("client.id", instance_id);
    }

    config
        .create::<FutureProducer>()
        .context("create relay future producer")
}

fn apply_security_settings(config: &mut ClientConfig, runtime: &RelayRuntimeConfig) {
    config.set("security.protocol", &runtime.security_protocol);

    if runtime.security_protocol == "ssl" {
        if let Some(value) = &runtime.tls_cert {
            config.set("ssl.certificate.location", value);
        }
        if let Some(value) = &runtime.tls_key {
            config.set("ssl.key.location", value);
        }
        if let Some(value) = &runtime.tls_ca {
            config.set("ssl.ca.location", value);
        }
    }

    if let Some(value) = &runtime.sasl_mechanism {
        config.set("sasl.mechanism", value);
    }
    if let Some(value) = &runtime.sasl_username {
        config.set("sasl.username", value);
    }
    if let Some(value) = &runtime.sasl_password {
        config.set("sasl.password", value);
    }
}

async fn process_message(
    consumer: &StreamConsumer,
    producer: &FutureProducer,
    runtime: &RelayRuntimeConfig,
    message: BorrowedMessage<'_>,
) -> Result<ProcessOutcome> {
    let payload = match message.payload() {
        Some(payload) => payload,
        None => {
            eprintln!(
                "hook relay skipped message without payload topic={} partition={} offset={}",
                message.topic(),
                message.partition(),
                message.offset()
            );
            return Ok(ProcessOutcome::DeferredNoCommit);
        }
    };

    if matches!(runtime.mode, RelayMode::Envelope)
        && serde_json::from_slice::<WebhookEnvelope>(payload).is_err()
    {
        eprintln!(
            "hook relay envelope validation failed topic={} partition={} offset={} (message left uncommitted)",
            message.topic(),
            message.partition(),
            message.offset()
        );
        return Ok(ProcessOutcome::DeferredNoCommit);
    }

    let key = message.key();

    publish_with_retry(
        producer,
        runtime,
        key,
        payload,
        message.topic(),
        message.partition(),
        message.offset(),
    )
    .await?;

    consumer
        .commit_message(&message, CommitMode::Async)
        .context("commit relay offset")?;

    Ok(ProcessOutcome::Committed)
}

async fn publish_with_retry(
    producer: &FutureProducer,
    runtime: &RelayRuntimeConfig,
    key: Option<&[u8]>,
    payload: &[u8],
    source_topic: &str,
    source_partition: i32,
    source_offset: i64,
) -> Result<()> {
    let mut attempt = 0u32;

    loop {
        let mut record = FutureRecord::to(&runtime.output_topic).payload(payload);
        if let Some(key_bytes) = key {
            record = record.key(key_bytes);
        }

        match producer
            .send(
                record,
                Timeout::After(Duration::from_secs(DEFAULT_KAFKA_MESSAGE_TIMEOUT_SECONDS)),
            )
            .await
        {
            Ok(_delivery) => return Ok(()),
            Err((error, _message)) => {
                attempt = attempt.saturating_add(1);
                if attempt >= runtime.max_retries {
                    return Err(anyhow!(
                        "relay publish failed source={source_topic}:{source_partition}:{source_offset} attempts={} error={}",
                        attempt,
                        error
                    ));
                }

                let backoff = retry_backoff_ms(
                    runtime.backoff_base_ms,
                    runtime.backoff_max_ms,
                    attempt.saturating_sub(1),
                );
                eprintln!(
                    "hook relay publish retry source={source_topic}:{source_partition}:{source_offset} attempt={} backoff_ms={} error={}",
                    attempt, backoff, error
                );
                sleep(Duration::from_millis(backoff)).await;
            }
        }
    }
}

fn retry_backoff_ms(base_ms: u64, max_ms: u64, attempt_index: u32) -> u64 {
    let exponent = attempt_index.min(31);
    let scaled = base_ms.saturating_mul(1u64 << exponent);
    scaled.min(max_ms)
}

fn parse_topics(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::retry_backoff_ms;

    #[test]
    fn retry_backoff_caps() {
        assert_eq!(retry_backoff_ms(100, 1000, 0), 100);
        assert_eq!(retry_backoff_ms(100, 1000, 1), 200);
        assert_eq!(retry_backoff_ms(100, 1000, 2), 400);
        assert_eq!(retry_backoff_ms(100, 1000, 3), 800);
        assert_eq!(retry_backoff_ms(100, 1000, 4), 1000);
        assert_eq!(retry_backoff_ms(100, 1000, 8), 1000);
    }
}
