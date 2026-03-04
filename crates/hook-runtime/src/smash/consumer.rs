use super::config::{Config, NoOutputSink, SmashPluginConfig, SmashRouteConfig};
use super::dlq::DlqProducer;
use crate::adapters::{RuntimeAdapter, build_runtime_adapters};
use anyhow::{Context, Result, anyhow};
use rdkafka::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message};
use relay_core::model::WebhookEnvelope;
use serde::Serialize;
use std::collections::BTreeMap;
use tracing::{Level, debug, error, info, warn};

const MAX_KAFKA_PAYLOAD_PREVIEW_CHARS: usize = 4_096;

pub struct KafkaConsumer {
    consumer: StreamConsumer,
    adapters: BTreeMap<String, RuntimeAdapter>,
    adapter_plugins: BTreeMap<String, Vec<SmashPluginConfig>>,
    smash_routes: Vec<SmashRouteConfig>,
    allow_no_output: bool,
    no_output_sink: Option<NoOutputSink>,
    dlq: DlqProducer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryOutcome {
    Commit,
    DoNotCommit,
}

impl KafkaConsumer {
    pub async fn from_config(config: &Config, dlq: DlqProducer) -> Result<Self> {
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

        let adapters = build_runtime_adapters(config).await?;
        let adapter_plugins = config
            .adapters
            .iter()
            .map(|adapter| (adapter.id().to_string(), adapter.plugins().to_vec()))
            .collect::<BTreeMap<_, _>>();

        Ok(Self {
            consumer,
            adapters,
            adapter_plugins,
            smash_routes: config.smash_routes.clone(),
            allow_no_output: config.allow_no_output,
            no_output_sink: config.no_output_sink,
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

        let delivery_outcome = self
            .deliver_to_routes(topic.as_str(), &envelope)
            .await
            .with_context(|| format!("deliver routed envelope event_id={}", envelope.id))?;

        if matches!(delivery_outcome, DeliveryOutcome::DoNotCommit) {
            warn!(
                topic = topic.as_str(),
                partition,
                offset,
                event_id = envelope.id.as_str(),
                "required destination failed; offset intentionally not committed"
            );
            return Ok(());
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

    async fn deliver_to_routes(
        &self,
        topic: &str,
        envelope: &WebhookEnvelope,
    ) -> Result<DeliveryOutcome> {
        let matched_routes = self
            .smash_routes
            .iter()
            .filter(|route| route_matches(route, topic, envelope.event_type.as_str()))
            .collect::<Vec<_>>();
        if matched_routes.is_empty() {
            return self
                .handle_no_output(
                    envelope,
                    format!(
                        "no matching smash route for topic '{}' and event '{}'",
                        topic, envelope.event_type
                    ),
                )
                .await;
        }

        let mut routed_destination_count = 0usize;
        for route in matched_routes {
            let required_destinations = route
                .destinations
                .iter()
                .filter(|destination| destination.required)
                .collect::<Vec<_>>();
            let optional_destinations = route
                .destinations
                .iter()
                .filter(|destination| !destination.required)
                .collect::<Vec<_>>();

            for destination in required_destinations {
                routed_destination_count = routed_destination_count.saturating_add(1);
                if let Err(error) = self
                    .deliver_destination(destination.adapter_id.as_str(), envelope)
                    .await
                {
                    let reason = format!(
                        "required destination adapter '{}' failed on route '{}': {}",
                        destination.adapter_id, route.id, error
                    );
                    warn!(
                        topic,
                        event_id = envelope.id.as_str(),
                        route_id = route.id.as_str(),
                        adapter_id = destination.adapter_id.as_str(),
                        error = %error,
                        "required destination failed"
                    );
                    self.dlq
                        .publish_failed(envelope, &reason)
                        .await
                        .context("publish required-delivery failure to dlq")?;
                    return Ok(DeliveryOutcome::DoNotCommit);
                }
            }

            for destination in optional_destinations {
                routed_destination_count = routed_destination_count.saturating_add(1);
                if let Err(error) = self
                    .deliver_destination(destination.adapter_id.as_str(), envelope)
                    .await
                {
                    warn!(
                        topic,
                        event_id = envelope.id.as_str(),
                        route_id = route.id.as_str(),
                        adapter_id = destination.adapter_id.as_str(),
                        error = %error,
                        "optional destination failed (continuing)"
                    );
                }
            }
        }

        if routed_destination_count == 0 {
            return self
                .handle_no_output(
                    envelope,
                    format!(
                        "no active smash destinations for topic '{}' and event '{}'",
                        topic, envelope.event_type
                    ),
                )
                .await;
        }

        Ok(DeliveryOutcome::Commit)
    }

    async fn deliver_destination(
        &self,
        adapter_id: &str,
        envelope: &WebhookEnvelope,
    ) -> Result<()> {
        let Some(adapter) = self.adapters.get(adapter_id) else {
            return Err(anyhow!("no adapter configured for '{}'", adapter_id));
        };
        let plugins = self
            .adapter_plugins
            .get(adapter_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let transformed_envelope = apply_smash_plugins(adapter_id, plugins, envelope)?;

        adapter.deliver(adapter_id, &transformed_envelope).await
    }

    async fn handle_no_output(
        &self,
        envelope: &WebhookEnvelope,
        reason: String,
    ) -> Result<DeliveryOutcome> {
        if !self.allow_no_output {
            return Err(anyhow!(reason));
        }

        match self.no_output_sink {
            Some(NoOutputSink::Discard) => {
                info!(
                    event_id = envelope.id.as_str(),
                    source = envelope.source.as_str(),
                    event_type = envelope.event_type.as_str(),
                    reason = reason.as_str(),
                    "allow_no_output=discard dropping message and committing offset"
                );
                Ok(DeliveryOutcome::Commit)
            }
            Some(NoOutputSink::Dlq) => {
                self.dlq
                    .publish_failed(envelope, &reason)
                    .await
                    .context("publish no-output event to dlq")?;
                Ok(DeliveryOutcome::Commit)
            }
            None => Err(anyhow!(
                "allow_no_output=true requires no_output_sink, but none configured"
            )),
        }
    }
}

fn apply_smash_plugins(
    adapter_id: &str,
    plugins: &[SmashPluginConfig],
    envelope: &WebhookEnvelope,
) -> Result<WebhookEnvelope> {
    if plugins.is_empty() {
        return Ok(envelope.clone());
    }

    let mut transformed = envelope.clone();
    for plugin in plugins {
        match plugin {
            SmashPluginConfig::EventTypeAlias { from, to } => {
                if transformed.event_type == from.as_str() {
                    transformed.event_type = to.clone();
                }
            }
            SmashPluginConfig::RequirePayloadField { pointer } => {
                if transformed.payload.pointer(pointer).is_none() {
                    return Err(anyhow!(
                        "smash adapter '{}' plugin require_payload_field missing '{}'",
                        adapter_id,
                        pointer
                    ));
                }
            }
            SmashPluginConfig::AddMetaFlag { flag } => {
                let meta = transformed.meta.get_or_insert_with(Default::default);
                if !meta.flags.iter().any(|existing| existing == flag) {
                    meta.flags.push(flag.clone());
                }
            }
        }
    }

    Ok(transformed)
}

fn route_matches(route: &SmashRouteConfig, topic: &str, event_type: &str) -> bool {
    wildcard_matches(route.source_topic_pattern.as_str(), topic)
        && route_event_filter_match(route, event_type)
}

fn route_event_filter_match(route: &SmashRouteConfig, event_type: &str) -> bool {
    if route.event_filters.is_empty() {
        return true;
    }

    route
        .event_filters
        .iter()
        .any(|filter| wildcard_matches(filter, event_type))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let normalized_pattern = pattern.trim();
    if normalized_pattern.is_empty() {
        return false;
    }
    if normalized_pattern == "*" {
        return true;
    }
    if !normalized_pattern.contains('*') {
        return normalized_pattern == value;
    }

    let mut remainder = value;
    let requires_prefix = !normalized_pattern.starts_with('*');
    let requires_suffix = !normalized_pattern.ends_with('*');
    let segments = normalized_pattern
        .split('*')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return true;
    }

    for (index, segment) in segments.iter().enumerate() {
        if index == 0 && requires_prefix {
            if !remainder.starts_with(segment) {
                return false;
            }
            remainder = &remainder[segment.len()..];
            continue;
        }

        if index == segments.len() - 1 && requires_suffix {
            return remainder.ends_with(segment);
        }

        match remainder.find(segment) {
            Some(position) => {
                let next_index = position + segment.len();
                remainder = &remainder[next_index..];
            }
            None => return false,
        }
    }

    true
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

#[cfg(test)]
mod tests {
    use super::{apply_smash_plugins, wildcard_matches};
    use crate::smash::config::SmashPluginConfig;
    use relay_core::model::{EventMeta, WebhookEnvelope};
    use serde_json::json;

    fn fixture_envelope() -> WebhookEnvelope {
        WebhookEnvelope {
            id: "evt-1".to_string(),
            source: "github".to_string(),
            event_type: "pull_request.opened".to_string(),
            received_at: "2026-03-04T00:00:00Z".to_string(),
            payload: json!({"action":"opened","repository":{"name":"repo"}}),
            meta: None,
        }
    }

    #[test]
    fn wildcard_matches_supports_basic_globs() {
        assert!(wildcard_matches("*", "webhooks.core"));
        assert!(wildcard_matches("webhooks.*", "webhooks.core"));
        assert!(wildcard_matches("*.core", "webhooks.core"));
        assert!(!wildcard_matches("webhooks.github", "webhooks.core"));
    }

    #[test]
    fn smash_plugins_alias_event_and_add_flag() {
        let envelope = fixture_envelope();
        let plugins = vec![
            SmashPluginConfig::EventTypeAlias {
                from: "pull_request.opened".to_string(),
                to: "pr.opened".to_string(),
            },
            SmashPluginConfig::AddMetaFlag {
                flag: "smash.plugin.alias".to_string(),
            },
        ];

        let transformed = apply_smash_plugins("openclaw-output", &plugins, &envelope)
            .expect("plugins should apply");
        assert_eq!(transformed.event_type, "pr.opened");
        assert_eq!(
            transformed.meta,
            Some(EventMeta {
                trace_id: None,
                ingress_adapter: None,
                route_key: None,
                flags: vec!["smash.plugin.alias".to_string()],
            })
        );
    }

    #[test]
    fn smash_plugins_require_payload_field_fail_closed() {
        let envelope = fixture_envelope();
        let plugins = vec![SmashPluginConfig::RequirePayloadField {
            pointer: "/missing".to_string(),
        }];

        let error =
            apply_smash_plugins("openclaw-output", &plugins, &envelope).expect_err("must fail");
        assert!(error.to_string().contains("/missing"));
    }
}
