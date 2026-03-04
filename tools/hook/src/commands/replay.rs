use crate::cli::{RelayMode, ReplayArgs, ReplayCommand, ReplayKafkaArgs, ReplayWebhookArgs};
use crate::config::AppContext;
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::util::Timeout;
use relay_core::model::WebhookEnvelope;
use reqwest::Client;
use std::fs;
use std::time::Duration;

const DEFAULT_REPLAY_TIMEOUT_SECONDS: u64 = 10;

pub async fn run(context: &AppContext, arguments: &ReplayArgs) -> Result<()> {
    match &arguments.command {
        ReplayCommand::Webhook(details) => replay_webhook(details).await,
        ReplayCommand::Kafka(details) => replay_kafka(context, details).await,
    }
}

async fn replay_webhook(arguments: &ReplayWebhookArgs) -> Result<()> {
    let body = fs::read_to_string(&arguments.file)
        .with_context(|| format!("read replay file: {}", arguments.file.display()))?;

    let mut url = arguments.url.clone();
    if let Some(source) = &arguments.source {
        let separator = if url.contains('?') { '&' } else { '?' };
        url = format!("{}{}source={}", url, separator, source);
    }

    let client = Client::new();
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(body);

    for header in &arguments.headers {
        let (name, value) = header
            .split_once(':')
            .ok_or_else(|| anyhow!("invalid header (expected Name:Value): {}", header))?;
        request = request.header(name.trim(), value.trim());
    }

    let response = request
        .send()
        .await
        .context("send replay webhook request")?;
    let status = response.status();
    let payload = response
        .text()
        .await
        .unwrap_or_else(|error| format!("unable to read response body: {error}"));

    println!("status={}", status);
    println!("body={}", payload);

    Ok(())
}

async fn replay_kafka(context: &AppContext, arguments: &ReplayKafkaArgs) -> Result<()> {
    let brokers = context
        .resolve_value(arguments.brokers.as_deref(), "KAFKA_BROKERS")
        .ok_or_else(|| anyhow!("missing KAFKA_BROKERS or --brokers"))?;

    let payload = fs::read(&arguments.file)
        .with_context(|| format!("read replay file: {}", arguments.file.display()))?;

    if matches!(arguments.mode, RelayMode::Envelope)
        && serde_json::from_slice::<WebhookEnvelope>(&payload).is_err()
    {
        return Err(anyhow!(
            "replay payload is not a valid WebhookEnvelope JSON in envelope mode"
        ));
    }

    let mut config = ClientConfig::new();
    config.set("bootstrap.servers", &brokers).set(
        "security.protocol",
        context
            .resolve_value(None, "KAFKA_SECURITY_PROTOCOL")
            .unwrap_or_else(|| "plaintext".to_string()),
    );

    if let Some(cert) = context.resolve_value(None, "KAFKA_TLS_CERT") {
        config.set("ssl.certificate.location", &cert);
    }
    if let Some(key) = context.resolve_value(None, "KAFKA_TLS_KEY") {
        config.set("ssl.key.location", &key);
    }
    if let Some(ca) = context.resolve_value(None, "KAFKA_TLS_CA") {
        config.set("ssl.ca.location", &ca);
    }
    if let Some(mechanism) = context.resolve_value(None, "KAFKA_SASL_MECHANISM") {
        config.set("sasl.mechanism", &mechanism);
    }
    if let Some(username) = context.resolve_value(None, "KAFKA_SASL_USERNAME") {
        config.set("sasl.username", &username);
    }
    if let Some(password) = context.resolve_value(None, "KAFKA_SASL_PASSWORD") {
        config.set("sasl.password", &password);
    }

    let producer = config
        .create::<FutureProducer>()
        .context("create replay producer")?;

    let mut record = FutureRecord::to(&arguments.topic).payload(&payload);
    if let Some(key) = &arguments.key {
        record = record.key(key);
    }

    producer
        .send(
            record,
            Timeout::After(Duration::from_secs(DEFAULT_REPLAY_TIMEOUT_SECONDS)),
        )
        .await
        .map_err(|(error, _)| anyhow!("kafka replay send failed: {error}"))?;

    println!(
        "replayed payload to kafka topic={} mode={:?}",
        arguments.topic, arguments.mode
    );

    Ok(())
}
