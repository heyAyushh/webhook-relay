use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub fn verify_github_signature(secret: &str, payload: &[u8], signature_header: &str) -> bool {
    let expected = compute_hmac_sha256_hex(secret, payload);
    let provided = normalize_signature(signature_header);
    constant_time_hex_equals(&provided, &expected)
}

pub fn verify_linear_signature(secret: &str, payload: &[u8], signature_header: &str) -> bool {
    let expected = compute_hmac_sha256_hex(secret, payload);
    let provided = normalize_signature(signature_header);
    constant_time_hex_equals(&provided, &expected)
}

pub fn compute_hmac_sha256_hex(secret: &str, payload: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts variable-length keys");
    mac.update(payload);
    hex::encode(mac.finalize().into_bytes())
}

fn normalize_signature(raw: &str) -> String {
    raw.trim()
        .strip_prefix("sha256=")
        .unwrap_or(raw.trim())
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn constant_time_hex_equals(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.as_bytes().ct_eq(right.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_github_signature_with_sha256_prefix() {
        let secret = "super-secret";
        let payload = br#"{"action":"opened"}"#;
        let digest = compute_hmac_sha256_hex(secret, payload);
        let header = format!("sha256={digest}");

        assert!(verify_github_signature(secret, payload, &header));
        assert!(!verify_github_signature(secret, payload, "sha256=deadbeef"));
    }

    #[test]
    fn verifies_linear_signature_without_prefix() {
        let secret = "linear-secret";
        let payload = br#"{"type":"Issue","action":"create"}"#;
        let digest = compute_hmac_sha256_hex(secret, payload);

        assert!(verify_linear_signature(secret, payload, &digest));
        assert!(verify_linear_signature(
            secret,
            payload,
            &format!("sha256={digest}")
        ));
        assert!(!verify_linear_signature(secret, payload, "deadbeef"));
    }
}
