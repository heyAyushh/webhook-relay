use crate::config::Config;
use crate::sources::{SourceHandler, ValidationError, header_value, payload_token};
use axum::http::HeaderMap;
use relay_core::keys::{github_cooldown_key, github_dedup_key};
use relay_core::signatures::verify_github_signature;
use serde_json::Value;

const GITHUB_SIGNATURE_HEADER: &str = "X-Hub-Signature-256";
const GITHUB_EVENT_HEADER: &str = "X-GitHub-Event";
const GITHUB_DELIVERY_HEADER: &str = "X-GitHub-Delivery";
const UNKNOWN_ACTION: &str = "unknown";
const GITHUB_SOURCE_NAME: &str = "github";
const MISSING_GITHUB_SECRET_MESSAGE: &str = "missing github secret";

#[derive(Debug, Default)]
pub struct GithubSourceHandler;

pub static HANDLER: GithubSourceHandler = GithubSourceHandler;

impl SourceHandler for GithubSourceHandler {
    fn source_name(&self) -> &'static str {
        GITHUB_SOURCE_NAME
    }

    fn validate_request(
        &self,
        config: &Config,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ValidationError> {
        let secret = config
            .hmac_secret_github
            .as_deref()
            .ok_or(ValidationError::Unauthorized(MISSING_GITHUB_SECRET_MESSAGE))?;
        validate(secret, headers, body)
    }

    fn event_type(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        event_type(headers, payload)
    }

    fn dedup_key(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
        let delivery_id = header_value(headers, GITHUB_DELIVERY_HEADER)
            .ok_or(ValidationError::BadRequest("missing X-GitHub-Delivery"))?;
        let action =
            payload_token(payload, &["action"]).unwrap_or_else(|| UNKNOWN_ACTION.to_string());
        let entity_id = entity_id(payload);
        Ok(github_dedup_key(&delivery_id, &action, &entity_id))
    }

    fn cooldown_key(&self, payload: &Value) -> Option<String> {
        let repo = payload_token(payload, &["repository", "full_name"])?;
        let entity_id = entity_id_for_cooldown(payload)?;
        Some(github_cooldown_key(&repo, &entity_id))
    }
}

pub fn validate(secret: &str, headers: &HeaderMap, body: &[u8]) -> Result<(), ValidationError> {
    let signature = header_string(headers, GITHUB_SIGNATURE_HEADER)
        .ok_or(ValidationError::Unauthorized("missing github signature"))?;

    if verify_github_signature(secret, body, &signature) {
        Ok(())
    } else {
        Err(ValidationError::Unauthorized("invalid github signature"))
    }
}

pub fn event_type(headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError> {
    let event_name = header_string(headers, GITHUB_EVENT_HEADER)
        .ok_or(ValidationError::BadRequest("missing X-GitHub-Event"))?;

    let action = payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();

    if action.is_empty() {
        Ok(event_name)
    } else {
        Ok(format!("{event_name}.{action}"))
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
        .or_else(|| payload_token(payload, &["comment", "id"]))
        .or_else(|| payload_token(payload, &["review", "id"]))
        .or_else(|| payload_token(payload, &["repository", "id"]))
        .unwrap_or_else(|| "unknown".to_string())
}

fn entity_id_for_cooldown(payload: &Value) -> Option<String> {
    payload_token(payload, &["pull_request", "number"])
        .or_else(|| payload_token(payload, &["issue", "number"]))
        .or_else(|| payload_token(payload, &["number"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use relay_core::signatures::compute_hmac_sha256_hex;
    use serde_json::json;

    #[test]
    fn validates_hmac_sha256_signature() {
        let secret = "github-secret";
        let body = br#"{"action":"opened"}"#;
        let digest = compute_hmac_sha256_hex(secret, body);

        let mut headers = HeaderMap::new();
        headers.insert(
            GITHUB_SIGNATURE_HEADER,
            HeaderValue::from_str(&format!("sha256={digest}")).expect("valid signature header"),
        );

        assert!(validate(secret, &headers, body).is_ok());
        assert!(validate("wrong", &headers, body).is_err());
    }

    #[test]
    fn extracts_event_type_with_action() {
        let mut headers = HeaderMap::new();
        headers.insert(
            GITHUB_EVENT_HEADER,
            HeaderValue::from_static("pull_request"),
        );

        let payload = json!({"action":"opened"});
        assert_eq!(
            event_type(&headers, &payload).expect("event type"),
            "pull_request.opened"
        );
    }

    #[test]
    fn accepts_arbitrary_event_and_action_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            GITHUB_EVENT_HEADER,
            HeaderValue::from_static("repository_dispatch"),
        );

        let payload = json!({"action":"custom_action"});
        assert_eq!(
            event_type(&headers, &payload).expect("event type"),
            "repository_dispatch.custom_action"
        );
    }

    #[test]
    fn accepts_event_without_action() {
        let mut headers = HeaderMap::new();
        headers.insert(GITHUB_EVENT_HEADER, HeaderValue::from_static("ping"));

        let payload = json!({});
        assert_eq!(event_type(&headers, &payload).expect("event type"), "ping");
    }

    #[test]
    fn accepts_all_documented_github_app_events() {
        // Source: https://docs.github.com/en/webhooks/webhook-events-and-payloads
        const DOCUMENTED_EVENTS: &[&str] = &[
            "branch_protection_configuration",
            "branch_protection_rule",
            "check_run",
            "check_suite",
            "code_scanning_alert",
            "commit_comment",
            "create",
            "custom_property",
            "custom_property_values",
            "delete",
            "dependabot_alert",
            "deploy_key",
            "deployment",
            "deployment_protection_rule",
            "deployment_review",
            "deployment_status",
            "discussion",
            "discussion_comment",
            "fork",
            "github_app_authorization",
            "gollum",
            "installation",
            "installation_repositories",
            "installation_target",
            "issue_comment",
            "issue_dependencies",
            "issues",
            "label",
            "marketplace_purchase",
            "member",
            "membership",
            "merge_group",
            "meta",
            "milestone",
            "org_block",
            "organization",
            "package",
            "page_build",
            "personal_access_token_request",
            "ping",
            "project",
            "project_card",
            "project_column",
            "projects_v2",
            "projects_v2_item",
            "projects_v2_status_update",
            "public",
            "pull_request",
            "pull_request_review",
            "pull_request_review_comment",
            "pull_request_review_thread",
            "push",
            "registry_package",
            "release",
            "repository",
            "repository_advisory",
            "repository_dispatch",
            "repository_import",
            "repository_ruleset",
            "repository_vulnerability_alert",
            "secret_scanning_alert",
            "secret_scanning_alert_location",
            "secret_scanning_scan",
            "security_advisory",
            "security_and_analysis",
            "sponsorship",
            "star",
            "status",
            "sub_issues",
            "team",
            "team_add",
            "watch",
            "workflow_dispatch",
            "workflow_job",
            "workflow_run",
        ];

        let payload = json!({});
        for event in DOCUMENTED_EVENTS {
            let mut headers = HeaderMap::new();
            headers.insert(
                GITHUB_EVENT_HEADER,
                HeaderValue::from_str(event).expect("valid github event header"),
            );

            assert_eq!(
                event_type(&headers, &payload).expect("event type"),
                *event,
                "failed for github event {event}"
            );
        }
    }

    #[test]
    fn builds_dedup_key_from_delivery_action_and_entity() {
        let mut headers = HeaderMap::new();
        headers.insert(
            GITHUB_DELIVERY_HEADER,
            HeaderValue::from_static("delivery-1"),
        );
        let payload = json!({"action":"opened","pull_request":{"number":42}});

        let key = HANDLER
            .dedup_key(&headers, &payload)
            .expect("github dedup key");
        assert_eq!(key, "github:delivery-1:opened:42");
    }

    #[test]
    fn builds_cooldown_key_from_repo_and_entity() {
        let payload = json!({
            "repository":{"full_name":"org/repo"},
            "pull_request":{"number":99}
        });
        assert_eq!(
            HANDLER.cooldown_key(&payload).as_deref(),
            Some("cooldown-github-org-repo-99")
        );
    }
}
