use serde_json::Value;

pub fn extract_linear_webhook_timestamp_epoch(payload: &Value) -> Option<i64> {
    let timestamp_value = payload.get("webhookTimestamp")?;
    let raw = if let Some(value) = timestamp_value.as_i64() {
        value
    } else if let Some(value) = timestamp_value.as_u64() {
        i64::try_from(value).ok()?
    } else if let Some(text) = timestamp_value.as_str() {
        text.parse::<i64>().ok()?
    } else {
        return None;
    };

    if raw > 10_000_000_000 {
        return Some(raw / 1000);
    }

    Some(raw)
}

pub fn verify_linear_timestamp_window(
    payload: &Value,
    now_epoch: i64,
    window_seconds: i64,
    enforce_check: bool,
) -> bool {
    if !enforce_check {
        return true;
    }

    let Some(ts) = extract_linear_webhook_timestamp_epoch(payload) else {
        return false;
    };

    (now_epoch - ts).abs() <= window_seconds
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_milliseconds_to_seconds() {
        let payload = json!({"webhookTimestamp": 1700000000123i64});
        assert_eq!(
            extract_linear_webhook_timestamp_epoch(&payload),
            Some(1700000000)
        );
    }

    #[test]
    fn accepts_timestamp_within_window() {
        let payload = json!({"webhookTimestamp": 1_700_000_000i64});
        assert!(verify_linear_timestamp_window(
            &payload,
            1_700_000_030,
            60,
            true
        ));
    }

    #[test]
    fn rejects_stale_timestamp_when_enforced() {
        let payload = json!({"webhookTimestamp": 1_700_000_000i64});
        assert!(!verify_linear_timestamp_window(
            &payload,
            1_700_000_500,
            60,
            true
        ));
    }

    #[test]
    fn bypasses_when_enforcement_disabled() {
        let payload = json!({});
        assert!(verify_linear_timestamp_window(
            &payload,
            1_700_000_500,
            60,
            false
        ));
    }
}
