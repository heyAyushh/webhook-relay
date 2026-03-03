use crate::config::Config;
use crate::sources::{SourceHandler, ValidationError, header_value, payload_token};
use axum::http::HeaderMap;
use relay_core::signatures::verify_shared_token;
use serde_json::Value;

const EXAMPLE_SOURCE_NAME: &str = "example";
const EXAMPLE_TOKEN_HEADER: &str = "X-Example-Token";
const EXAMPLE_EVENT_HEADER: &str = "X-Example-Event";
const EXAMPLE_DELIVERY_HEADER: &str = "X-Example-Delivery";
const MISSING_EXAMPLE_SECRET_MESSAGE: &str = "missing example secret";
const MISSING_EXAMPLE_TOKEN_MESSAGE: &str = "missing example token";
const INVALID_EXAMPLE_TOKEN_MESSAGE: &str = "invalid example token";
const MISSING_EXAMPLE_EVENT_MESSAGE: &str = "missing example event";
const MISSING_EXAMPLE_DELIVERY_MESSAGE: &str = "missing X-Example-Delivery";
const UNKNOWN_ACTION_TOKEN: &str = "unknown";
const UNKNOWN_ENTITY_TOKEN: &str = "unknown";

#[derive(Debug, Default)]
pub struct ExampleSourceHandler;

pub static HANDLER: ExampleSourceHandler = ExampleSourceHandler;

impl SourceHandler for ExampleSourceHandler {
    fn source_name(&self) -> &'static str {
        EXAMPLE_SOURCE_NAME
    }

    fn validate_request(
        &self,
        config: &Config,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ValidationError> {
        let secret = config
            .hmac_secret_example
            .as_deref()
            .ok_or(ValidationError::Unauthorized(
                MISSING_EXAMPLE_SECRET_MESSAGE,
            ))?;
        validate(secret, headers, body)
    }

    fn event_type(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        event_type(headers, payload)
    }

    fn dedup_key(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        let delivery_id = header_value(headers, EXAMPLE_DELIVERY_HEADER).ok_or(
            ValidationError::BadRequest(MISSING_EXAMPLE_DELIVERY_MESSAGE),
        )?;
        let action =
            payload_token(payload, &["action"]).unwrap_or_else(|| UNKNOWN_ACTION_TOKEN.to_string());
        let entity_id = entity_id(payload);
        Ok(format!("example:{delivery_id}:{action}:{entity_id}"))
    }

    fn cooldown_key(&self, payload: &Value) -> Option<String> {
        let scope = payload_token(payload, &["scope"])
            .or_else(|| payload_token(payload, &["tenant"]))
            .or_else(|| payload_token(payload, &["project", "id"]))?;
        let entity_id = entity_id_for_cooldown(payload)?;
        Some(format!("cooldown-example-{scope}-{entity_id}"))
    }
}

pub fn validate(secret: &str, headers: &HeaderMap, _body: &[u8]) -> Result<(), ValidationError> {
    let token = header_value(headers, EXAMPLE_TOKEN_HEADER)
        .ok_or(ValidationError::Unauthorized(MISSING_EXAMPLE_TOKEN_MESSAGE))?;
    if verify_shared_token(secret, &token) {
        Ok(())
    } else {
        Err(ValidationError::Unauthorized(INVALID_EXAMPLE_TOKEN_MESSAGE))
    }
}

pub fn event_type(headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
    let event = header_value(headers, EXAMPLE_EVENT_HEADER)
        .or_else(|| payload_token(payload, &["event_type"]))
        .or_else(|| payload_token(payload, &["type"]))
        .ok_or(ValidationError::BadRequest(MISSING_EXAMPLE_EVENT_MESSAGE))?;

    let action = payload_token(payload, &["action"]);
    let normalized_event = event.to_ascii_lowercase();
    match action {
        Some(action) => Ok(format!(
            "{}.{}",
            normalized_event,
            action.to_ascii_lowercase()
        )),
        None => Ok(normalized_event),
    }
}

fn entity_id(payload: &Value) -> String {
    entity_id_for_cooldown(payload)
        .or_else(|| payload_token(payload, &["resource", "id"]))
        .unwrap_or_else(|| UNKNOWN_ENTITY_TOKEN.to_string())
}

fn entity_id_for_cooldown(payload: &Value) -> Option<String> {
    payload_token(payload, &["data", "id"]).or_else(|| payload_token(payload, &["id"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;

    #[test]
    fn validates_shared_token() {
        let secret = "example-secret";
        let body = br#"{"type":"ticket"}"#;
        let mut headers = HeaderMap::new();
        headers.insert(
            EXAMPLE_TOKEN_HEADER,
            HeaderValue::from_static("example-secret"),
        );

        assert!(validate(secret, &headers, body).is_ok());
        assert!(validate("different", &headers, body).is_err());
    }

    #[test]
    fn builds_event_type_from_header_and_action() {
        let mut headers = HeaderMap::new();
        headers.insert(EXAMPLE_EVENT_HEADER, HeaderValue::from_static("Ticket"));
        let payload = json!({"action":"Open"});
        assert_eq!(
            event_type(&headers, &payload).expect("example event type"),
            "ticket.open"
        );
    }

    #[test]
    fn builds_dedup_key_from_delivery_action_and_entity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            EXAMPLE_DELIVERY_HEADER,
            HeaderValue::from_static("delivery-7"),
        );
        let payload = json!({"action":"create","data":{"id":"task-1"}});

        let key = HANDLER
            .dedup_key(&headers, &payload)
            .expect("example dedup key");
        assert_eq!(key, "example:delivery-7:create:task-1");
    }

    #[test]
    fn builds_cooldown_key_from_scope_and_entity() {
        let payload = json!({"scope":"workspace-1","data":{"id":"task-1"}});
        assert_eq!(
            HANDLER.cooldown_key(&payload).as_deref(),
            Some("cooldown-example-workspace-1-task-1")
        );
    }
}
