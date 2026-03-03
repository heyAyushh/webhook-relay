use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

pub const DEFAULT_SOURCE_TOPIC_PREFIX: &str = "webhooks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Github,
    Linear,
}

impl Source {
    // Built-in example source names.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Github => "github",
            Source::Linear => "linear",
        }
    }

    pub fn topic_name(self) -> &'static str {
        match self {
            Source::Github => "webhooks.github",
            Source::Linear => "webhooks.linear",
        }
    }
}

pub fn normalize_source_name(source: &str) -> Option<String> {
    let normalized = source.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub fn source_topic_name(source_topic_prefix: &str, source: &str) -> Option<String> {
    let prefix = source_topic_prefix.trim();
    if prefix.is_empty() {
        return None;
    }
    normalize_source_name(source).map(|normalized| format!("{prefix}.{normalized}"))
}

impl FromStr for Source {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Source::Github),
            "linear" => Ok(Source::Linear),
            _ => Err("unsupported source"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookEnvelope {
    pub id: String,
    pub source: String,
    pub event_type: String,
    pub received_at: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEnvelope {
    pub failed_at: String,
    pub error: String,
    pub envelope: WebhookEnvelope,
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_SOURCE_TOPIC_PREFIX, normalize_source_name, source_topic_name};

    #[test]
    fn normalizes_source_name() {
        assert_eq!(normalize_source_name(" GitHub ").as_deref(), Some("github"));
        assert!(normalize_source_name(" ").is_none());
    }

    #[test]
    fn composes_topic_name_from_prefix_and_source() {
        assert_eq!(
            source_topic_name(DEFAULT_SOURCE_TOPIC_PREFIX, "Linear").as_deref(),
            Some("webhooks.linear")
        );
        assert!(source_topic_name("", "linear").is_none());
    }
}
