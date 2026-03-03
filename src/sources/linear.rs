use crate::config::Config;
use crate::sources::{SourceHandler, ValidationError, header_value, payload_token};
use axum::http::HeaderMap;
use relay_core::keys::{linear_cooldown_key, linear_dedup_key};
use relay_core::signatures::verify_linear_signature;
use relay_core::timestamps::verify_linear_timestamp_window;
use serde_json::Value;

const LINEAR_SIGNATURE_HEADER: &str = "Linear-Signature";
const LINEAR_DELIVERY_HEADER: &str = "Linear-Delivery";
const UNKNOWN_ACTION: &str = "unknown";
const LINEAR_SOURCE_NAME: &str = "linear";
const MISSING_LINEAR_SECRET_MESSAGE: &str = "missing linear secret";

#[derive(Debug, Default)]
pub struct LinearSourceHandler;

pub static HANDLER: LinearSourceHandler = LinearSourceHandler;

impl SourceHandler for LinearSourceHandler {
    fn source_name(&self) -> &'static str {
        LINEAR_SOURCE_NAME
    }

    fn validate_request(
        &self,
        config: &Config,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ValidationError> {
        let secret = config
            .hmac_secret_linear
            .as_deref()
            .ok_or(ValidationError::Unauthorized(MISSING_LINEAR_SECRET_MESSAGE))?;
        validate(secret, headers, body)
    }

    fn validate_payload(
        &self,
        config: &Config,
        payload: &Value,
        now_epoch_seconds: i64,
    ) -> Result<(), ValidationError> {
        if verify_linear_timestamp_window(
            payload,
            now_epoch_seconds,
            config.linear_timestamp_window_seconds,
            config.enforce_linear_timestamp_window,
        ) {
            Ok(())
        } else {
            Err(ValidationError::Unauthorized(
                "linear webhook rejected due to timestamp window check",
            ))
        }
    }

    fn event_type(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        event_type(headers, payload)
    }

    fn dedup_key(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        let delivery_id = header_value(headers, LINEAR_DELIVERY_HEADER)
            .ok_or(ValidationError::BadRequest("missing Linear-Delivery"))?;
        let action =
            payload_token(payload, &["action"]).unwrap_or_else(|| UNKNOWN_ACTION.to_string());
        let entity_id = entity_id(payload);
        Ok(linear_dedup_key(&delivery_id, &action, &entity_id))
    }

    fn cooldown_key(&self, payload: &Value) -> Option<String> {
        let team_key = payload_token(payload, &["data", "team", "key"])?;
        let entity_id = entity_id_for_cooldown(payload)?;
        Some(linear_cooldown_key(&team_key, &entity_id))
    }
}

pub fn validate(secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), ValidationError> {
    let signature = header_string(headers, LINEAR_SIGNATURE_HEADER)
        .ok_or(ValidationError::Unauthorized("missing linear signature"))?;

    if verify_linear_signature(secret, body, &signature) {
        Ok(())
    } else {
        Err(ValidationError::Unauthorized("invalid linear signature"))
    }
}

pub fn event_type(headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
    let linear_type = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| header_string(headers, "Linear-Event"))
        .ok_or(ValidationError::BadRequest("missing linear type"))?;

    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let normalized_type = linear_type.to_ascii_lowercase();
    match action {
        Some(action) => Ok(format!(
            "{}.{}",
            normalized_type,
            action.to_ascii_lowercase()
        )),
        None => Ok(normalized_type),
    }
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn entity_id(payload: &Value) -> String {
    entity_id_for_cooldown(payload)
        .or_else(|| payload_token(payload, &["webhookId"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn entity_id_for_cooldown(payload: &Value) -> Option<String> {
    payload_token(payload, &["data", "id"])
        .or_else(|| payload_token(payload, &["data", "identifier"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use relay_core::signatures::compute_hmac_sha256_hex;
    use serde_json::json;

    #[test]
    fn validates_hmac_signature() {
        let secret = "linear-secret";
        let body = br#"{"type":"Issue","action":"create"}"#;
        let digest = compute_hmac_sha256_hex(secret, body);

        let mut headers = HeaderMap::new();
        headers.insert(
            LINEAR_SIGNATURE_HEADER,
            HeaderValue::from_str(&digest).expect("valid digest header"),
        );

        assert!(validate(secret, &headers, body).is_ok());
        assert!(validate("wrong", &headers, body).is_err());
    }

    #[test]
    fn extracts_type_and_action() {
        let headers = HeaderMap::new();
        let payload = json!({"type":"Issue","action":"create"});
        assert_eq!(
            event_type(&headers, &payload).expect("linear event type"),
            "issue.create"
        );
    }

    #[test]
    fn accepts_type_without_action() {
        let headers = HeaderMap::new();
        let payload = json!({"type":"Project"});
        assert_eq!(
            event_type(&headers, &payload).expect("linear event type"),
            "project"
        );
    }

    #[test]
    fn accepts_arbitrary_type_and_action_values() {
        let headers = HeaderMap::new();
        let payload = json!({"type":"RoadmapUpdate","action":"Archived"});
        assert_eq!(
            event_type(&headers, &payload).expect("linear event type"),
            "roadmapupdate.archived"
        );
    }

    #[test]
    fn falls_back_to_linear_event_header_when_type_is_missing() {
        let mut headers = HeaderMap::new();
        headers.insert("Linear-Event", HeaderValue::from_static("Issue"));

        let payload = json!({"action":"create"});
        assert_eq!(
            event_type(&headers, &payload).expect("linear event type"),
            "issue.create"
        );
    }

    #[test]
    fn accepts_all_documented_linear_webhook_types() {
        // Sources:
        // - https://linear.app/developers/webhooks
        // - https://studio.apollographql.com/public/Linear-Webhooks/variant/current/schema/reference/objects
        const DOCUMENTED_TYPES: &[&str] = &[
            "Comment",
            "Cycle",
            "Customer",
            "CustomerRequest",
            "Document",
            "Initiative",
            "InitiativeUpdate",
            "Issue",
            "IssueAttachment",
            "IssueLabel",
            "IssueSLA",
            "OAuthApp",
            "Project",
            "ProjectUpdate",
            "Reaction",
            "User",
        ];

        let headers = HeaderMap::new();
        for linear_type in DOCUMENTED_TYPES {
            let payload = json!({"type": linear_type, "action":"create"});
            assert_eq!(
                event_type(&headers, &payload).expect("linear event type"),
                format!("{}.create", linear_type.to_ascii_lowercase()),
                "failed for linear type {linear_type}"
            );
        }
    }

    #[test]
    fn builds_dedup_key_from_delivery_action_and_entity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINEAR_DELIVERY_HEADER,
            HeaderValue::from_static("delivery-2"),
        );
        let payload = json!({"action":"create","data":{"id":"issue-42"}});

        let key = HANDLER
            .dedup_key(&headers, &payload)
            .expect("linear dedup key");
        assert_eq!(key, "linear:delivery-2:create:issue-42");
    }

    #[test]
    fn builds_cooldown_key_from_team_and_entity() {
        let payload = json!({
            "data":{"team":{"key":"ENG"},"id":"issue-42"}
        });
        assert_eq!(
            HANDLER.cooldown_key(&payload).as_deref(),
            Some("cooldown-linear-ENG-issue-42")
        );
    }
}
