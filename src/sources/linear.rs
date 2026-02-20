use crate::sources::ValidationError;
use axum::http::HeaderMap;
use relay_core::signatures::verify_linear_signature;
use serde_json::Value;

const LINEAR_SIGNATURE_HEADER: &str = "Linear-Signature";

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
}
