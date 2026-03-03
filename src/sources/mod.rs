use crate::config::Config;
use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

pub mod example;
pub mod github;
pub mod linear;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationError {
    Unauthorized(&'static str),
    BadRequest(&'static str),
}

pub trait SourceHandler: Sync {
    fn source_name(&self) -> &'static str;

    fn topic_name(&self, config: &Config) -> String {
        config.source_topic_name(self.source_name())
    }

    fn validate_request(
        &self,
        config: &Config,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Result<(), ValidationError>;

    fn validate_payload(
        &self,
        _config: &Config,
        _payload: &Value,
        _now_epoch_seconds: i64,
    ) -> Result<(), ValidationError> {
        Ok(())
    }

    fn event_type(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError>;

    fn dedup_key(&self, headers: &HeaderMap, payload: &Value) -> Result<String, ValidationError>;

    fn cooldown_key(&self, payload: &Value) -> Option<String>;
}

static SOURCE_HANDLERS: LazyLock<HashMap<&'static str, &'static dyn SourceHandler>> =
    LazyLock::new(|| {
        let mut handlers: HashMap<&'static str, &'static dyn SourceHandler> = HashMap::new();
        handlers.insert(example::HANDLER.source_name(), &example::HANDLER);
        handlers.insert(github::HANDLER.source_name(), &github::HANDLER);
        handlers.insert(linear::HANDLER.source_name(), &linear::HANDLER);
        handlers
    });

pub fn handler_for_source(source: &str) -> Option<&'static dyn SourceHandler> {
    let normalized = normalize_source_name(source)?;
    SOURCE_HANDLERS.get(normalized.as_str()).copied()
}

pub fn has_handler(source: &str) -> bool {
    handler_for_source(source).is_some()
}

pub fn known_source_names() -> Vec<&'static str> {
    let mut names = SOURCE_HANDLERS.keys().copied().collect::<Vec<_>>();
    names.sort_unstable();
    names
}

pub fn normalize_source_name(source: &str) -> Option<String> {
    let normalized = source.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub(crate) fn payload_token(payload: &Value, path: &[&str]) -> Option<String> {
    let mut current = payload;
    for segment in path {
        current = current.get(*segment)?;
    }

    if let Some(value) = current.as_str() {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else if let Some(value) = current.as_i64() {
        Some(value.to_string())
    } else {
        current.as_u64().map(|value| value.to_string())
    }
}

pub(crate) fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::{known_source_names, normalize_source_name};

    #[test]
    fn normalizes_source_names() {
        assert_eq!(normalize_source_name(" GitHub ").as_deref(), Some("github"));
        assert!(normalize_source_name("   ").is_none());
    }

    #[test]
    fn includes_builtin_sources() {
        let names = known_source_names();
        assert!(names.contains(&"example"));
        assert!(names.contains(&"github"));
        assert!(names.contains(&"linear"));
    }
}
