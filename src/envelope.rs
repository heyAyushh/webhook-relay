use chrono::{SecondsFormat, Utc};
use relay_core::model::{Source, WebhookEnvelope};
use serde_json::Value;
use uuid::Uuid;

pub fn build_envelope(source: Source, event_type: String, payload: Value) -> WebhookEnvelope {
    WebhookEnvelope {
        id: Uuid::new_v4().to_string(),
        source: source.as_str().to_string(),
        event_type,
        received_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        payload,
    }
}
