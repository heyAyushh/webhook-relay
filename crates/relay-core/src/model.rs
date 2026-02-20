use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Github,
    Linear,
}

impl Source {
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
