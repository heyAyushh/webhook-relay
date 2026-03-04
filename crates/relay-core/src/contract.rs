use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;
use toml::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppContract {
    pub app: AppMeta,
    #[serde(default)]
    pub policies: Policies,
    pub serve: ServeSection,
    pub smash: SmashSection,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileDef>,
    #[serde(default)]
    pub mcp: Option<Value>,
    #[serde(default)]
    pub websocket: Option<Value>,
    #[serde(default)]
    pub transports: BTreeMap<String, TransportDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationMode {
    #[default]
    Strict,
    Debug,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoOutputSink {
    Discard,
    Dlq,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policies {
    #[serde(default)]
    pub allow_no_output: bool,
    #[serde(default)]
    pub no_output_sink: Option<NoOutputSink>,
    #[serde(default)]
    pub validation_mode: ValidationMode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeSection {
    #[serde(default)]
    pub ingress_adapters: Vec<IngressAdapter>,
    #[serde(default)]
    pub routes: Vec<ServeRoute>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmashSection {
    #[serde(default)]
    pub egress_adapters: Vec<EgressAdapter>,
    #[serde(default)]
    pub routes: Vec<SmashRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressDriver {
    HttpWebhookIngress,
    WebsocketIngress,
    McpIngestExposed,
    KafkaIngress,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressDriver {
    OpenclawHttpOutput,
    McpToolOutput,
    WebsocketClientOutput,
    WebsocketServerOutput,
    KafkaOutput,
    Unknown(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct IngressAdapter {
    pub id: String,
    pub driver: IngressDriver,
    #[serde(flatten, default)]
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EgressAdapter {
    pub id: String,
    pub driver: EgressDriver,
    #[serde(flatten, default)]
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServeRoute {
    pub id: String,
    pub source_match: String,
    pub event_type_pattern: String,
    pub target_topic: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmashRoute {
    pub id: String,
    pub source_topic_pattern: String,
    #[serde(default)]
    pub event_filters: Vec<String>,
    pub destinations: Vec<RouteDestination>,
}

fn default_required_destination() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDestination {
    pub adapter_id: String,
    #[serde(default = "default_required_destination")]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDef {
    pub label: String,
    #[serde(default)]
    pub serve_adapters: Vec<String>,
    #[serde(default)]
    pub smash_adapters: Vec<String>,
    #[serde(default)]
    pub serve_routes: Vec<String>,
    #[serde(default)]
    pub smash_routes: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportDriver {
    StdioJsonrpc,
    HttpSse,
    Unknown(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransportDef {
    pub driver: TransportDriver,
    #[serde(flatten, default)]
    pub config: BTreeMap<String, Value>,
}

pub fn parse_contract(content: &str) -> Result<AppContract, toml::de::Error> {
    toml::from_str::<AppContract>(content)
}

impl IngressDriver {
    pub fn as_str(&self) -> &str {
        match self {
            Self::HttpWebhookIngress => "http_webhook_ingress",
            Self::WebsocketIngress => "websocket_ingress",
            Self::McpIngestExposed => "mcp_ingest_exposed",
            Self::KafkaIngress => "kafka_ingress",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl EgressDriver {
    pub fn as_str(&self) -> &str {
        match self {
            Self::OpenclawHttpOutput => "openclaw_http_output",
            Self::McpToolOutput => "mcp_tool_output",
            Self::WebsocketClientOutput => "websocket_client_output",
            Self::WebsocketServerOutput => "websocket_server_output",
            Self::KafkaOutput => "kafka_output",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl TransportDriver {
    pub fn as_str(&self) -> &str {
        match self {
            Self::StdioJsonrpc => "stdio_jsonrpc",
            Self::HttpSse => "http_sse",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for IngressDriver {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "http_webhook_ingress" => Self::HttpWebhookIngress,
            "websocket_ingress" => Self::WebsocketIngress,
            "mcp_ingest_exposed" => Self::McpIngestExposed,
            "kafka_ingress" => Self::KafkaIngress,
            _ => Self::Unknown(raw),
        })
    }
}

impl<'de> Deserialize<'de> for EgressDriver {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "openclaw_http_output" => Self::OpenclawHttpOutput,
            "mcp_tool_output" => Self::McpToolOutput,
            "websocket_client_output" => Self::WebsocketClientOutput,
            "websocket_server_output" => Self::WebsocketServerOutput,
            "kafka_output" => Self::KafkaOutput,
            _ => Self::Unknown(raw),
        })
    }
}

impl<'de> Deserialize<'de> for TransportDriver {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "stdio_jsonrpc" => Self::StdioJsonrpc,
            "http_sse" => Self::HttpSse,
            _ => Self::Unknown(raw),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EgressDriver, IngressDriver, NoOutputSink, TransportDriver, ValidationMode, parse_contract,
    };

    #[test]
    fn parses_minimal_contract_with_profiles() {
        let contract = parse_contract(
            r#"
[app]
id = "default-openclaw"
name = "Default OpenClaw"
version = "1.0.0"

[serve]

[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"

[[serve.routes]]
id = "all-to-core"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]

[[smash.egress_adapters]]
id = "openclaw-output"
driver = "openclaw_http_output"

[[smash.routes]]
id = "core-to-openclaw"
source_topic_pattern = "webhooks.core"

destinations = [{ adapter_id = "openclaw-output" }]

[profiles.default-openclaw]
label = "Default"
serve_adapters = ["http-ingress"]
smash_adapters = ["openclaw-output"]
serve_routes = ["all-to-core"]
smash_routes = ["core-to-openclaw"]
"#,
        )
        .expect("parse contract");

        assert_eq!(contract.app.id, "default-openclaw");
        assert_eq!(contract.policies.validation_mode, ValidationMode::Strict);
        assert_eq!(contract.policies.no_output_sink, None);
        assert_eq!(contract.policies.allow_no_output, false);
    }

    #[test]
    fn parses_policies_no_output_sink() {
        let contract = parse_contract(
            r#"
[app]
id = "x"
name = "x"
version = "1.0.0"

[policies]
allow_no_output = true
no_output_sink = "dlq"

[serve]

[[serve.ingress_adapters]]
id = "http-ingress"
driver = "http_webhook_ingress"

[[serve.routes]]
id = "r1"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]

[[smash.egress_adapters]]
id = "k"
driver = "kafka_output"

[[smash.routes]]
id = "r2"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "k" }]

[profiles.default-openclaw]
label = "Default"
serve_adapters = ["http-ingress"]
smash_adapters = ["k"]
serve_routes = ["r1"]
smash_routes = ["r2"]
"#,
        )
        .expect("parse contract with sink");

        assert_eq!(contract.policies.no_output_sink, Some(NoOutputSink::Dlq));
    }

    #[test]
    fn preserves_unknown_drivers_in_contract() {
        let contract = parse_contract(
            r#"
[app]
id = "x"
name = "x"
version = "1.0.0"

[serve]
[[serve.ingress_adapters]]
id = "i1"
driver = "custom_ingress"

[[serve.routes]]
id = "r1"
source_match = "*"
event_type_pattern = "*"
target_topic = "webhooks.core"

[smash]
[[smash.egress_adapters]]
id = "e1"
driver = "custom_output"

[[smash.routes]]
id = "r2"
source_topic_pattern = "webhooks.core"
destinations = [{ adapter_id = "e1" }]

[transports.demo]
driver = "custom_transport"

[profiles.default-openclaw]
label = "Default"
serve_adapters = []
smash_adapters = []
serve_routes = []
smash_routes = []
"#,
        )
        .expect("parse contract with unknown drivers");

        assert!(matches!(
            contract.serve.ingress_adapters[0].driver,
            IngressDriver::Unknown(_)
        ));
        assert!(matches!(
            contract.smash.egress_adapters[0].driver,
            EgressDriver::Unknown(_)
        ));
        assert!(matches!(
            contract.transports["demo"].driver,
            TransportDriver::Unknown(_)
        ));
    }
}
