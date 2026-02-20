use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    pub fn openclaw_source_query(self) -> &'static str {
        match self {
            Source::Github => "github-pr",
            Source::Linear => "linear",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingEvent {
    pub event_id: String,
    pub source: Source,
    pub dedup_key: String,
    pub cooldown_key: String,
    pub action: String,
    pub entity_id: String,
    pub payload: Value,
    pub metadata: EventMetadata,
    pub attempts: u32,
    pub next_retry_at_epoch: i64,
    pub created_at_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMetadata {
    pub delivery_id: String,
    pub event_name: Option<String>,
    pub installation_id: Option<String>,
    pub team_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqEvent {
    pub pending_event: PendingEvent,
    pub failure_reason: String,
    pub failed_at_epoch: i64,
    pub replay_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueResult {
    Enqueued,
    Duplicate,
    Cooldown,
}
