use crate::contract::{
    AppContract, EgressAdapter, EgressDriver, IngressAdapter, IngressDriver, NoOutputSink,
    ProfileDef, SmashRoute, TransportDef, TransportDriver, ValidationMode,
};
use std::collections::{BTreeMap, BTreeSet};
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub code: &'static str,
    pub message: String,
    pub security_critical: bool,
}

#[derive(Debug, Clone)]
pub struct ValidatedProfile {
    pub profile_name: String,
    pub validation_mode: ValidationMode,
    pub serve_adapter_ids: Vec<String>,
    pub smash_adapter_ids: Vec<String>,
    pub serve_route_ids: Vec<String>,
    pub smash_route_ids: Vec<String>,
    pub skipped_non_security_checks: Vec<String>,
}

pub fn validate_contract(
    contract: &AppContract,
    profile_name: &str,
) -> Result<ValidatedProfile, Vec<ValidationError>> {
    let mut errors = Vec::new();

    let profile = match contract.profiles.get(profile_name) {
        Some(profile) => profile,
        None => {
            return Err(vec![ValidationError {
                code: "missing_profile",
                message: format!("profile '{profile_name}' not found in contract"),
                security_critical: true,
            }]);
        }
    };

    let validation_mode = contract.policies.validation_mode;
    let mut skipped_non_security_checks = Vec::new();

    let serve_adapters_by_id = index_ingress_adapters(&contract.serve.ingress_adapters);
    let smash_adapters_by_id = index_egress_adapters(&contract.smash.egress_adapters);
    let serve_routes_by_id =
        index_routes(contract.serve.routes.iter().map(|route| route.id.as_str()));
    let smash_routes_by_id =
        index_routes(contract.smash.routes.iter().map(|route| route.id.as_str()));

    validate_profile_refs(
        profile,
        &serve_adapters_by_id,
        &smash_adapters_by_id,
        &serve_routes_by_id,
        &smash_routes_by_id,
        &mut errors,
    );

    let active_ingress_adapters = profile
        .serve_adapters
        .iter()
        .filter_map(|adapter_id| serve_adapters_by_id.get(adapter_id.as_str()).copied())
        .collect::<Vec<_>>();
    let active_egress_adapters = profile
        .smash_adapters
        .iter()
        .filter_map(|adapter_id| smash_adapters_by_id.get(adapter_id.as_str()).copied())
        .collect::<Vec<_>>();
    let active_transport_refs = collect_active_transport_refs(&active_egress_adapters);
    let active_smash_routes = contract
        .smash
        .routes
        .iter()
        .filter(|route| {
            profile
                .smash_routes
                .iter()
                .any(|active| active == &route.id)
        })
        .collect::<Vec<_>>();

    validate_ingress_adapter_schemas(&active_ingress_adapters, &mut errors);
    validate_egress_adapter_schemas(contract, &active_egress_adapters, &mut errors);
    validate_transport_schemas(contract, &active_transport_refs, &mut errors);
    validate_smash_route_destinations(
        &active_smash_routes,
        &profile.smash_adapters,
        &smash_adapters_by_id,
        &mut errors,
    );
    validate_no_output_policy(contract, profile, &active_smash_routes, &mut errors);

    maybe_validate_non_security(
        validation_mode,
        profile,
        &mut errors,
        &mut skipped_non_security_checks,
    );

    if errors.is_empty() {
        return Ok(ValidatedProfile {
            profile_name: profile_name.to_string(),
            validation_mode,
            serve_adapter_ids: profile.serve_adapters.clone(),
            smash_adapter_ids: profile.smash_adapters.clone(),
            serve_route_ids: profile.serve_routes.clone(),
            smash_route_ids: profile.smash_routes.clone(),
            skipped_non_security_checks,
        });
    }

    Err(errors)
}

fn index_ingress_adapters(adapters: &[IngressAdapter]) -> BTreeMap<&str, &IngressAdapter> {
    let mut by_id = BTreeMap::new();
    for adapter in adapters {
        by_id.insert(adapter.id.as_str(), adapter);
    }
    by_id
}

fn index_egress_adapters(adapters: &[EgressAdapter]) -> BTreeMap<&str, &EgressAdapter> {
    let mut by_id = BTreeMap::new();
    for adapter in adapters {
        by_id.insert(adapter.id.as_str(), adapter);
    }
    by_id
}

fn index_routes<'a>(route_ids: impl Iterator<Item = &'a str>) -> BTreeSet<&'a str> {
    route_ids.collect::<BTreeSet<_>>()
}

fn validate_profile_refs(
    profile: &ProfileDef,
    serve_adapters_by_id: &BTreeMap<&str, &IngressAdapter>,
    smash_adapters_by_id: &BTreeMap<&str, &EgressAdapter>,
    serve_routes_by_id: &BTreeSet<&str>,
    smash_routes_by_id: &BTreeSet<&str>,
    errors: &mut Vec<ValidationError>,
) {
    for adapter_id in &profile.serve_adapters {
        if !serve_adapters_by_id.contains_key(adapter_id.as_str()) {
            errors.push(ValidationError {
                code: "missing_serve_adapter",
                message: format!("profile references unknown serve adapter '{adapter_id}'"),
                security_critical: true,
            });
        }
    }

    for adapter_id in &profile.smash_adapters {
        if !smash_adapters_by_id.contains_key(adapter_id.as_str()) {
            errors.push(ValidationError {
                code: "missing_smash_adapter",
                message: format!("profile references unknown smash adapter '{adapter_id}'"),
                security_critical: true,
            });
        }
    }

    for route_id in &profile.serve_routes {
        if !serve_routes_by_id.contains(route_id.as_str()) {
            errors.push(ValidationError {
                code: "missing_serve_route",
                message: format!("profile references unknown serve route '{route_id}'"),
                security_critical: true,
            });
        }
    }

    for route_id in &profile.smash_routes {
        if !smash_routes_by_id.contains(route_id.as_str()) {
            errors.push(ValidationError {
                code: "missing_smash_route",
                message: format!("profile references unknown smash route '{route_id}'"),
                security_critical: true,
            });
        }
    }
}

fn validate_ingress_adapter_schemas(
    adapters: &[&IngressAdapter],
    errors: &mut Vec<ValidationError>,
) {
    for adapter in adapters {
        match adapter.driver {
            IngressDriver::HttpWebhookIngress => {
                validate_adapter_config_schema(
                    "serve",
                    adapter.id.as_str(),
                    "http_webhook_ingress",
                    &adapter.config,
                    &["bind"],
                    &["path_template", "plugins"],
                    errors,
                );
            }
            IngressDriver::WebsocketIngress => {
                validate_adapter_config_schema(
                    "serve",
                    adapter.id.as_str(),
                    "websocket_ingress",
                    &adapter.config,
                    &["auth_mode"],
                    &["path_template", "plugins"],
                    errors,
                );
            }
            IngressDriver::McpIngestExposed => {
                validate_adapter_config_schema(
                    "serve",
                    adapter.id.as_str(),
                    "mcp_ingest_exposed",
                    &adapter.config,
                    &["transport_driver", "bind", "auth_mode", "max_payload_bytes"],
                    &["tool_name", "path", "token_env", "plugins"],
                    errors,
                );
            }
            IngressDriver::KafkaIngress => {
                validate_adapter_config_schema(
                    "serve",
                    adapter.id.as_str(),
                    "kafka_ingress",
                    &adapter.config,
                    &["topics", "group_id"],
                    &["brokers", "plugins"],
                    errors,
                );
            }
            IngressDriver::Unknown(_) => {
                errors.push(ValidationError {
                    code: "unsupported_ingress_driver",
                    message: format!(
                        "serve adapter '{}' uses unsupported driver '{}'",
                        adapter.id,
                        adapter.driver.as_str()
                    ),
                    security_critical: true,
                });
            }
        }
    }
}

fn validate_egress_adapter_schemas(
    contract: &AppContract,
    adapters: &[&EgressAdapter],
    errors: &mut Vec<ValidationError>,
) {
    for adapter in adapters {
        match adapter.driver {
            EgressDriver::OpenclawHttpOutput => {
                validate_adapter_config_schema(
                    "smash",
                    adapter.id.as_str(),
                    "openclaw_http_output",
                    &adapter.config,
                    &["url", "token_env", "timeout_seconds", "max_retries"],
                    &["plugins"],
                    errors,
                );
            }
            EgressDriver::McpToolOutput => {
                validate_adapter_config_schema(
                    "smash",
                    adapter.id.as_str(),
                    "mcp_tool_output",
                    &adapter.config,
                    &["tool_name", "transport_ref"],
                    &["plugins"],
                    errors,
                );
                match adapter
                    .config
                    .get("transport_ref")
                    .and_then(as_trimmed_str)
                    .filter(|value| !value.is_empty())
                {
                    Some(transport_ref) => {
                        if !contract.transports.contains_key(transport_ref) {
                            errors.push(ValidationError {
                                code: "missing_transport_ref",
                                message: format!(
                                    "smash adapter '{}' references unknown transport '{}'",
                                    adapter.id, transport_ref
                                ),
                                security_critical: true,
                            });
                        }
                    }
                    None => {
                        errors.push(ValidationError {
                            code: "missing_transport_ref",
                            message: format!(
                                "smash adapter '{}' has invalid transport_ref",
                                adapter.id
                            ),
                            security_critical: true,
                        });
                    }
                }
            }
            EgressDriver::WebsocketClientOutput => {
                validate_adapter_config_schema(
                    "smash",
                    adapter.id.as_str(),
                    "websocket_client_output",
                    &adapter.config,
                    &["url", "auth_mode", "send_timeout_ms", "retry_policy"],
                    &["plugins"],
                    errors,
                );
            }
            EgressDriver::WebsocketServerOutput => {
                validate_adapter_config_schema(
                    "smash",
                    adapter.id.as_str(),
                    "websocket_server_output",
                    &adapter.config,
                    &[
                        "bind",
                        "path",
                        "auth_mode",
                        "max_clients",
                        "queue_depth_per_client",
                        "send_timeout_ms",
                    ],
                    &["plugins"],
                    errors,
                );
            }
            EgressDriver::KafkaOutput => {
                validate_adapter_config_schema(
                    "smash",
                    adapter.id.as_str(),
                    "kafka_output",
                    &adapter.config,
                    &["topic", "key_mode"],
                    &["plugins"],
                    errors,
                );
            }
            EgressDriver::Unknown(_) => {
                errors.push(ValidationError {
                    code: "unsupported_egress_driver",
                    message: format!(
                        "smash adapter '{}' uses unsupported driver '{}'",
                        adapter.id,
                        adapter.driver.as_str()
                    ),
                    security_critical: true,
                });
            }
        }
    }
}

fn collect_active_transport_refs(active_egress_adapters: &[&EgressAdapter]) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    for adapter in active_egress_adapters {
        if matches!(adapter.driver, EgressDriver::McpToolOutput) {
            if let Some(transport_ref) =
                adapter.config.get("transport_ref").and_then(as_trimmed_str)
            {
                if !transport_ref.is_empty() {
                    refs.insert(transport_ref.to_string());
                }
            }
        }
    }
    refs
}

fn validate_transport_schemas(
    contract: &AppContract,
    active_transport_refs: &BTreeSet<String>,
    errors: &mut Vec<ValidationError>,
) {
    for transport_name in active_transport_refs {
        let Some(transport) = contract.transports.get(transport_name) else {
            continue;
        };
        validate_transport_config_schema(transport_name, transport, errors);
    }
}

fn validate_transport_config_schema(
    transport_name: &str,
    transport: &TransportDef,
    errors: &mut Vec<ValidationError>,
) {
    match transport.driver {
        TransportDriver::StdioJsonrpc => validate_adapter_config_schema(
            "transport",
            transport_name,
            "stdio_jsonrpc",
            &transport.config,
            &[],
            &["command", "args", "env"],
            errors,
        ),
        TransportDriver::HttpSse => validate_adapter_config_schema(
            "transport",
            transport_name,
            "http_sse",
            &transport.config,
            &["url", "auth_mode"],
            &[],
            errors,
        ),
        TransportDriver::Unknown(_) => errors.push(ValidationError {
            code: "unsupported_transport_driver",
            message: format!(
                "transport '{}' uses unsupported driver '{}'",
                transport_name,
                transport.driver.as_str()
            ),
            security_critical: true,
        }),
    }
}

fn validate_adapter_config_schema(
    kind: &str,
    id: &str,
    driver_name: &str,
    config: &BTreeMap<String, Value>,
    required_keys: &[&str],
    optional_keys: &[&str],
    errors: &mut Vec<ValidationError>,
) {
    let allowed_keys = required_keys
        .iter()
        .chain(optional_keys.iter())
        .copied()
        .collect::<BTreeSet<_>>();

    for key in config.keys() {
        if !allowed_keys.contains(key.as_str()) {
            errors.push(ValidationError {
                code: "unknown_adapter_key",
                message: format!(
                    "{kind} adapter '{id}' (driver '{driver_name}') has unknown key '{key}'"
                ),
                security_critical: true,
            });
        }
    }

    for required_key in required_keys {
        match config.get(*required_key) {
            Some(value) if is_non_empty_toml(value) => {}
            Some(_) => errors.push(ValidationError {
                code: "empty_required_adapter_value",
                message: format!(
                    "{kind} adapter '{id}' (driver '{driver_name}') has empty required key '{required_key}'"
                ),
                security_critical: true,
            }),
            None => errors.push(ValidationError {
                code: "missing_required_adapter_key",
                message: format!(
                    "{kind} adapter '{id}' (driver '{driver_name}') is missing required key '{required_key}'"
                ),
                security_critical: true,
            }),
        }
    }
}

fn validate_smash_route_destinations(
    routes: &[&SmashRoute],
    active_smash_adapters: &[String],
    smash_adapters_by_id: &BTreeMap<&str, &EgressAdapter>,
    errors: &mut Vec<ValidationError>,
) {
    for route in routes {
        if route.destinations.is_empty() {
            errors.push(ValidationError {
                code: "empty_smash_route_destinations",
                message: format!("smash route '{}' has no destinations", route.id),
                security_critical: true,
            });
            continue;
        }

        for destination in &route.destinations {
            if !active_smash_adapters
                .iter()
                .any(|adapter_id| adapter_id == &destination.adapter_id)
            {
                errors.push(ValidationError {
                    code: "inactive_destination_adapter",
                    message: format!(
                        "smash route '{}' references adapter '{}' that is not active in profile",
                        route.id, destination.adapter_id
                    ),
                    security_critical: true,
                });
                continue;
            }

            if !smash_adapters_by_id.contains_key(destination.adapter_id.as_str()) {
                errors.push(ValidationError {
                    code: "missing_destination_adapter",
                    message: format!(
                        "smash route '{}' references unknown adapter '{}'",
                        route.id, destination.adapter_id
                    ),
                    security_critical: true,
                });
            }
        }
    }
}

fn validate_no_output_policy(
    contract: &AppContract,
    profile: &ProfileDef,
    active_smash_routes: &[&SmashRoute],
    errors: &mut Vec<ValidationError>,
) {
    let active_destination_count = active_smash_routes
        .iter()
        .flat_map(|route| route.destinations.iter())
        .filter(|destination| {
            profile
                .smash_adapters
                .iter()
                .any(|adapter_id| adapter_id == &destination.adapter_id)
        })
        .count();
    if active_destination_count > 0 {
        return;
    }

    if !contract.policies.allow_no_output {
        errors.push(ValidationError {
            code: "no_smash_outputs",
            message: "profile has zero active smash outputs and allow_no_output is false"
                .to_string(),
            security_critical: true,
        });
        return;
    }

    if contract.policies.no_output_sink.is_none() {
        errors.push(ValidationError {
            code: "missing_no_output_sink",
            message: "allow_no_output=true requires no_output_sink to be set".to_string(),
            security_critical: true,
        });
    }

    if matches!(contract.policies.no_output_sink, Some(NoOutputSink::Dlq))
        && profile.smash_routes.is_empty()
    {
        errors.push(ValidationError {
            code: "dlq_without_routes",
            message: "no_output_sink=dlq requires at least one active smash route".to_string(),
            security_critical: true,
        });
    }
}

fn maybe_validate_non_security(
    validation_mode: ValidationMode,
    profile: &ProfileDef,
    errors: &mut Vec<ValidationError>,
    skipped: &mut Vec<String>,
) {
    if matches!(validation_mode, ValidationMode::Debug) {
        skipped.push("non_security_profile_label_check".to_string());
        skipped.push("non_security_profile_env_table_check".to_string());
        return;
    }

    if profile.label.trim().is_empty() {
        errors.push(ValidationError {
            code: "empty_profile_label",
            message: "profile label cannot be empty".to_string(),
            security_critical: false,
        });
    }
}

fn is_non_empty_toml(value: &Value) -> bool {
    match value {
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Table(value) => !value.is_empty(),
        _ => true,
    }
}

fn as_trimmed_str(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value.trim()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::validate_contract;
    use crate::contract::parse_contract;

    fn fixture() -> String {
        r#"
[app]
id = "default-openclaw"
name = "Default OpenClaw"
version = "1.0.0"

[policies]
validation_mode = "strict"
allow_no_output = false

[serve]

[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"
path_template = "/webhook/{source}"

[[serve.routes]]
id = "all-to-core"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]

[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"
url = "http://127.0.0.1:18789/hooks/agent"
token_env = "OPENCLAW_WEBHOOK_TOKEN"
timeout_seconds = 20
max_retries = 5

[[smash.routes]]
id = "core-to-openclaw"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "openclaw-output", required = true }]

[profiles.default-openclaw]
label = "Default OpenClaw"
serve_adapters = ["http-ingress"]
smash_adapters = ["openclaw-output"]
serve_routes = ["all-to-core"]
smash_routes = ["core-to-openclaw"]
"#
        .to_string()
    }

    #[test]
    fn validates_happy_path_profile() {
        let contract = parse_contract(&fixture()).expect("parse contract");
        let validated = validate_contract(&contract, "default-openclaw").expect("validate");
        assert_eq!(validated.serve_adapter_ids, vec!["http-ingress"]);
        assert_eq!(validated.smash_adapter_ids, vec!["openclaw-output"]);
    }

    #[test]
    fn rejects_missing_profile() {
        let contract = parse_contract(&fixture()).expect("parse contract");
        let error = validate_contract(&contract, "missing").expect_err("missing profile error");
        assert_eq!(error[0].code, "missing_profile");
    }

    #[test]
    fn rejects_unknown_adapter_key() {
        let contract = parse_contract(&fixture().replace(
            "path_template = \"/webhook/{source}\"",
            "path_template = \"/webhook/{source}\"\nextra = \"nope\"",
        ))
        .expect("parse contract");
        let errors = validate_contract(&contract, "default-openclaw").expect_err("invalid");
        assert!(
            errors
                .iter()
                .any(|error| error.code == "unknown_adapter_key")
        );
    }

    #[test]
    fn debug_mode_relaxes_non_security_checks() {
        let contract = parse_contract(
            &fixture()
                .replace(
                    "validation_mode = \"strict\"",
                    "validation_mode = \"debug\"",
                )
                .replace("label = \"Default OpenClaw\"", "label = \"  \""),
        )
        .expect("parse contract");

        let validated = validate_contract(&contract, "default-openclaw").expect("valid");
        assert!(
            validated
                .skipped_non_security_checks
                .iter()
                .any(|check| check == "non_security_profile_label_check")
        );
    }

    #[test]
    fn debug_mode_still_rejects_security_critical_missing_keys() {
        let contract = parse_contract(
            &fixture()
                .replace(
                    "validation_mode = \"strict\"",
                    "validation_mode = \"debug\"",
                )
                .replace("token_env = \"OPENCLAW_WEBHOOK_TOKEN\"\n", ""),
        )
        .expect("parse contract");
        let errors = validate_contract(&contract, "default-openclaw").expect_err("invalid");
        assert!(
            errors.iter().any(
                |error| error.code == "missing_required_adapter_key" && error.security_critical
            )
        );
    }

    #[test]
    fn allows_unknown_drivers_when_inactive_in_profile() {
        let contract_text = format!(
            "{}\n\n[[serve.ingress_adapters]]\nid = \"custom-ingress\"\ndriver = \"custom_ingress\"\n\n[[smash.egress_adapters]]\nid = \"custom-output\"\ndriver = \"custom_output\"\n\n[transports.unused]\ndriver = \"custom_transport\"\n",
            fixture()
        );
        let contract = parse_contract(&contract_text).expect("parse contract");

        let validated = validate_contract(&contract, "default-openclaw").expect("validate");
        assert_eq!(validated.serve_adapter_ids, vec!["http-ingress"]);
        assert_eq!(validated.smash_adapter_ids, vec!["openclaw-output"]);
    }

    #[test]
    fn rejects_unknown_drivers_when_active_in_profile() {
        let contract_text = format!(
            "{}\n\n[[serve.ingress_adapters]]\nid = \"custom-ingress\"\ndriver = \"custom_ingress\"\n",
            fixture().replace(
                "serve_adapters = [\"http-ingress\"]",
                "serve_adapters = [\"http-ingress\", \"custom-ingress\"]"
            )
        );
        let contract = parse_contract(&contract_text).expect("parse contract");
        let errors = validate_contract(&contract, "default-openclaw").expect_err("invalid");

        assert!(
            errors
                .iter()
                .any(|error| error.code == "unsupported_ingress_driver")
        );
    }

    #[test]
    fn rejects_unknown_transport_driver_when_referenced_by_active_adapter() {
        let contract = parse_contract(
            r#"
[app]
id = "default-openclaw"
name = "Default OpenClaw"
version = "1.0.0"

[policies]
validation_mode = "strict"
allow_no_output = false

[serve]
[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"
bind = "0.0.0.0:8080"

[[serve.routes]]
id = "all-to-core"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]
[[smash.egress_adapters]]
id = "mcp-output"
driver = "mcp_tool_output"
tool_name = "emit_event"
transport_ref = "main"

[[smash.routes]]
id = "core-to-mcp"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "mcp-output", required = true }]

[transports.main]
driver = "custom_transport"

[profiles.default-openclaw]
label = "Default OpenClaw"
serve_adapters = ["http-ingress"]
smash_adapters = ["mcp-output"]
serve_routes = ["all-to-core"]
smash_routes = ["core-to-mcp"]
"#,
        )
        .expect("parse contract");
        let errors = validate_contract(&contract, "default-openclaw").expect_err("invalid");

        assert!(
            errors
                .iter()
                .any(|error| error.code == "unsupported_transport_driver")
        );
    }
}
