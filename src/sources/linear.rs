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

pub fn event_type(payload: &Value) -> Result<String, ValidationError> {
    let linear_type = payload
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ValidationError::BadRequest("missing linear type"))?;

    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ValidationError::BadRequest("missing linear action"))?;

    Ok(format!("{}.{}", linear_type.to_ascii_lowercase(), action))
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
        let payload = json!({"type":"Issue","action":"create"});
        assert_eq!(
            event_type(&payload).expect("linear event type"),
            "issue.create"
        );
    }
}
