use crate::sources::ValidationError;
use axum::http::HeaderMap;
use relay_core::signatures::verify_shared_token;
use serde_json::Value;

const GMAIL_TOKEN_HEADER: &str = "X-Goog-Token";
const GMAIL_STATE_HEADER: &str = "X-Goog-Resource-State";

pub fn validate(secret: &str, headers: &HeaderMap) -> Result<(), ValidationError> {
    let token = header_string(headers, GMAIL_TOKEN_HEADER)
        .ok_or(ValidationError::Unauthorized("missing gmail token"))?;

    if verify_shared_token(secret, &token) {
        Ok(())
    } else {
        Err(ValidationError::Unauthorized("invalid gmail token"))
    }
}

pub fn event_type(headers: &HeaderMap, payload: &Value) -> String {
    if let Some(event_type) = payload
        .get("event_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return event_type.to_string();
    }

    if let Some(resource_state) = header_string(headers, GMAIL_STATE_HEADER) {
        return format!("gmail.{}", resource_state.to_ascii_lowercase());
    }

    "gmail.event".to_string()
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
    use serde_json::json;

    #[test]
    fn validates_token_header() {
        let mut headers = HeaderMap::new();
        headers.insert(GMAIL_TOKEN_HEADER, HeaderValue::from_static("gmail-token"));

        assert!(validate("gmail-token", &headers).is_ok());
        assert!(validate("wrong", &headers).is_err());
    }

    #[test]
    fn derives_event_type_from_resource_state() {
        let mut headers = HeaderMap::new();
        headers.insert(GMAIL_STATE_HEADER, HeaderValue::from_static("exists"));

        let payload = json!({});
        assert_eq!(event_type(&headers, &payload), "gmail.exists");
    }
}
