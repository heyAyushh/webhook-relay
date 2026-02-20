use crate::model::{DlqEvent, EnqueueResult, PendingEvent};
use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;

const PENDING_EVENTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("pending_events");
const DLQ_EVENTS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("dlq_events");
const DEDUP_INDEX_TABLE: TableDefinition<&str, i64> = TableDefinition::new("dedup_index");
const COOLDOWN_INDEX_TABLE: TableDefinition<&str, i64> = TableDefinition::new("cooldown_index");

#[derive(Debug, Clone)]
pub struct RelayStore {
    db: Arc<Database>,
}

impl RelayStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db directory at {}", parent.display()))?;
        }

        let db =
            Database::create(path).with_context(|| format!("open redb at {}", path.display()))?;
        let write_tx = db
            .begin_write()
            .context("begin write transaction for table init")?;
        {
            write_tx
                .open_table(PENDING_EVENTS_TABLE)
                .context("open pending events table")?;
            write_tx
                .open_table(DLQ_EVENTS_TABLE)
                .context("open dlq events table")?;
            write_tx
                .open_table(DEDUP_INDEX_TABLE)
                .context("open dedup index table")?;
            write_tx
                .open_table(COOLDOWN_INDEX_TABLE)
                .context("open cooldown index table")?;
        }
        write_tx.commit().context("commit table init transaction")?;

        Ok(Self { db: Arc::new(db) })
    }

    pub fn enqueue_pending_event(
        &self,
        event: PendingEvent,
        dedup_retention_seconds: i64,
        cooldown_seconds: i64,
        now_epoch: i64,
    ) -> Result<EnqueueResult> {
        let write_tx = self
            .db
            .begin_write()
            .context("begin write transaction for enqueue")?;

        {
            let mut dedup_index = write_tx
                .open_table(DEDUP_INDEX_TABLE)
                .context("open dedup table")?;
            if let Some(existing_expiry) = dedup_index
                .get(event.dedup_key.as_str())
                .context("read dedup key")?
            {
                if existing_expiry.value() > now_epoch {
                    return Ok(EnqueueResult::Duplicate);
                }
            }

            let dedup_expires_at = now_epoch + dedup_retention_seconds;
            dedup_index
                .insert(event.dedup_key.as_str(), dedup_expires_at)
                .context("insert dedup key")?;
        }

        let cooldown_hit = {
            let mut cooldown_index = write_tx
                .open_table(COOLDOWN_INDEX_TABLE)
                .context("open cooldown table")?;
            let existing_expiry = cooldown_index
                .get(event.cooldown_key.as_str())
                .context("read cooldown key")?
                .map(|guard| guard.value());

            if matches!(existing_expiry, Some(expiry) if expiry > now_epoch) {
                true
            } else {
                let cooldown_expires_at = now_epoch + cooldown_seconds;
                cooldown_index
                    .insert(event.cooldown_key.as_str(), cooldown_expires_at)
                    .context("upsert cooldown key")?;
                false
            }
        };

        if cooldown_hit {
            write_tx
                .commit()
                .context("commit enqueue transaction after cooldown")?;
            return Ok(EnqueueResult::Cooldown);
        }

        {
            let mut pending_events = write_tx
                .open_table(PENDING_EVENTS_TABLE)
                .context("open pending table")?;
            let serialized = serialize_json(&event).context("serialize pending event")?;
            pending_events
                .insert(event.event_id.as_str(), serialized.as_str())
                .context("insert pending event")?;
        }

        write_tx.commit().context("commit enqueue transaction")?;

        Ok(EnqueueResult::Enqueued)
    }

    pub fn pop_due_event(&self, now_epoch: i64) -> Result<Option<PendingEvent>> {
        let write_tx = self
            .db
            .begin_write()
            .context("begin write transaction for pop_due_event")?;

        let mut selected_id: Option<String> = None;
        let mut selected_event: Option<PendingEvent> = None;

        {
            let pending_events = write_tx
                .open_table(PENDING_EVENTS_TABLE)
                .context("open pending table")?;
            let iter = pending_events.iter().context("iterate pending events")?;

            for entry in iter {
                let (event_id_guard, payload_guard) = entry.context("read pending row")?;
                let event_id = event_id_guard.value();
                let payload = payload_guard.value();
                let event: PendingEvent = deserialize_json(payload)
                    .with_context(|| format!("deserialize pending event {event_id}"))?;

                if event.next_retry_at_epoch <= now_epoch {
                    match &selected_event {
                        Some(current_best)
                            if event.next_retry_at_epoch >= current_best.next_retry_at_epoch => {}
                        _ => {
                            selected_id = Some(event_id.to_string());
                            selected_event = Some(event);
                        }
                    }
                }
            }
        }

        if let Some(event_id) = selected_id {
            {
                let mut pending_events = write_tx
                    .open_table(PENDING_EVENTS_TABLE)
                    .context("open pending table for delete")?;
                pending_events
                    .remove(event_id.as_str())
                    .context("remove popped event")?;
            }
            write_tx.commit().context("commit pop transaction")?;
            return Ok(selected_event);
        }

        drop(write_tx);
        Ok(None)
    }

    pub fn requeue_event(&self, event: PendingEvent) -> Result<()> {
        let write_tx = self
            .db
            .begin_write()
            .context("begin write transaction for requeue")?;
        {
            let mut pending_events = write_tx
                .open_table(PENDING_EVENTS_TABLE)
                .context("open pending table")?;
            let serialized = serialize_json(&event).context("serialize requeue event")?;
            pending_events
                .insert(event.event_id.as_str(), serialized.as_str())
                .context("insert requeued event")?;
        }
        write_tx.commit().context("commit requeue transaction")?;
        Ok(())
    }

    pub fn move_to_dlq(&self, event: PendingEvent, reason: &str, now_epoch: i64) -> Result<()> {
        let write_tx = self
            .db
            .begin_write()
            .context("begin write transaction for move_to_dlq")?;

        let dlq_event = DlqEvent {
            pending_event: event.clone(),
            failure_reason: reason.to_string(),
            failed_at_epoch: now_epoch,
            replay_count: 0,
        };

        {
            let mut dlq_events = write_tx
                .open_table(DLQ_EVENTS_TABLE)
                .context("open dlq table")?;
            let serialized = serialize_json(&dlq_event).context("serialize dlq event")?;
            dlq_events
                .insert(event.event_id.as_str(), serialized.as_str())
                .context("insert dlq event")?;
        }

        write_tx.commit().context("commit move_to_dlq")?;
        Ok(())
    }

    pub fn replay_dlq_event(&self, event_id: &str, now_epoch: i64) -> Result<bool> {
        let write_tx = self
            .db
            .begin_write()
            .context("begin write transaction for replay")?;

        let maybe_replay_event = {
            let mut dlq_events = write_tx
                .open_table(DLQ_EVENTS_TABLE)
                .context("open dlq table")?;

            let maybe_raw = dlq_events
                .get(event_id)
                .context("read dlq event")?
                .map(|entry| entry.value().to_string());

            let Some(raw) = maybe_raw else {
                return Ok(false);
            };

            let mut dlq_event: DlqEvent =
                deserialize_json(&raw).context("deserialize dlq event for replay")?;
            dlq_event.replay_count += 1;

            let mut replay_event = dlq_event.pending_event;
            replay_event.attempts = 0;
            replay_event.next_retry_at_epoch = now_epoch;

            dlq_events
                .remove(event_id)
                .context("remove dlq event for replay")?;

            replay_event
        };

        {
            let mut pending_events = write_tx
                .open_table(PENDING_EVENTS_TABLE)
                .context("open pending table for replay")?;
            let serialized =
                serialize_json(&maybe_replay_event).context("serialize replay event")?;
            pending_events
                .insert(event_id, serialized.as_str())
                .context("insert replayed event")?;
        }

        write_tx.commit().context("commit replay transaction")?;
        Ok(true)
    }

    pub fn pending_count(&self) -> Result<usize> {
        let read_tx = self
            .db
            .begin_read()
            .context("begin read transaction for pending_count")?;
        let pending_events = read_tx
            .open_table(PENDING_EVENTS_TABLE)
            .context("open pending table")?;
        Ok(pending_events
            .iter()
            .context("iterate pending for count")?
            .count())
    }

    pub fn dlq_count(&self) -> Result<usize> {
        let read_tx = self
            .db
            .begin_read()
            .context("begin read transaction for dlq_count")?;
        let dlq_events = read_tx
            .open_table(DLQ_EVENTS_TABLE)
            .context("open dlq table")?;
        Ok(dlq_events.iter().context("iterate dlq for count")?.count())
    }

    pub fn list_dlq_events(&self, limit: usize) -> Result<Vec<DlqEvent>> {
        let read_tx = self
            .db
            .begin_read()
            .context("begin read transaction for list_dlq")?;
        let dlq_events = read_tx
            .open_table(DLQ_EVENTS_TABLE)
            .context("open dlq table")?;

        let mut events = Vec::new();
        let iter = dlq_events.iter().context("iterate dlq table")?;
        for (index, entry) in iter.enumerate() {
            if index >= limit {
                break;
            }
            let (_event_id_guard, payload_guard) = entry.context("read dlq row")?;
            let payload = payload_guard.value();
            events.push(deserialize_json(payload).context("deserialize dlq row")?);
        }

        Ok(events)
    }
}

fn serialize_json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).context("serialize JSON")
}

fn deserialize_json<T: for<'de> serde::Deserialize<'de>>(raw: &str) -> Result<T> {
    serde_json::from_str(raw).context("deserialize JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EventMetadata, Source};
    use serde_json::json;
    use tempfile::TempDir;

    fn sample_event(
        event_id: &str,
        dedup_key: &str,
        cooldown_key: &str,
        now_epoch: i64,
    ) -> PendingEvent {
        PendingEvent {
            event_id: event_id.to_string(),
            source: Source::Github,
            dedup_key: dedup_key.to_string(),
            cooldown_key: cooldown_key.to_string(),
            action: "opened".to_string(),
            entity_id: "42".to_string(),
            payload: json!({"action":"opened"}),
            metadata: EventMetadata {
                delivery_id: "delivery-1".to_string(),
                event_name: Some("pull_request".to_string()),
                installation_id: Some("123".to_string()),
                team_key: None,
            },
            attempts: 0,
            next_retry_at_epoch: now_epoch,
            created_at_epoch: now_epoch,
        }
    }

    #[test]
    fn returns_duplicate_when_same_dedup_key_within_retention() {
        let tmp = TempDir::new().expect("tempdir");
        let store = RelayStore::open(&tmp.path().join("relay.redb")).expect("store");
        let now = 1_700_000_000;

        let first = sample_event(
            "event-1",
            "github:d:a:e",
            "cooldown-github-org-repo-42",
            now,
        );
        let second = sample_event(
            "event-2",
            "github:d:a:e",
            "cooldown-github-org-repo-42",
            now + 1,
        );

        assert_eq!(
            store
                .enqueue_pending_event(first, 7 * 24 * 60 * 60, 30, now)
                .expect("enqueue first"),
            EnqueueResult::Enqueued
        );
        assert_eq!(
            store
                .enqueue_pending_event(second, 7 * 24 * 60 * 60, 30, now + 1)
                .expect("enqueue second"),
            EnqueueResult::Duplicate
        );
    }

    #[test]
    fn returns_cooldown_for_different_delivery_same_entity() {
        let tmp = TempDir::new().expect("tempdir");
        let store = RelayStore::open(&tmp.path().join("relay.redb")).expect("store");
        let now = 1_700_000_000;

        let first = sample_event(
            "event-1",
            "github:d1:opened:42",
            "cooldown-github-org-repo-42",
            now,
        );
        let second = sample_event(
            "event-2",
            "github:d2:opened:42",
            "cooldown-github-org-repo-42",
            now + 5,
        );

        assert_eq!(
            store
                .enqueue_pending_event(first, 7 * 24 * 60 * 60, 30, now)
                .expect("enqueue first"),
            EnqueueResult::Enqueued
        );
        assert_eq!(
            store
                .enqueue_pending_event(second, 7 * 24 * 60 * 60, 30, now + 5)
                .expect("enqueue second"),
            EnqueueResult::Cooldown
        );
    }

    #[test]
    fn pops_due_event_and_moves_to_dlq() {
        let tmp = TempDir::new().expect("tempdir");
        let store = RelayStore::open(&tmp.path().join("relay.redb")).expect("store");
        let now = 1_700_000_000;

        let event = sample_event(
            "event-1",
            "github:d1:opened:42",
            "cooldown-github-org-repo-42",
            now,
        );
        assert_eq!(
            store
                .enqueue_pending_event(event.clone(), 7 * 24 * 60 * 60, 30, now)
                .expect("enqueue"),
            EnqueueResult::Enqueued
        );

        let popped = store
            .pop_due_event(now)
            .expect("pop due")
            .expect("expected due event");
        assert_eq!(popped.event_id, "event-1");

        store
            .move_to_dlq(popped, "forward_failed", now + 10)
            .expect("move to dlq");
        assert_eq!(store.pending_count().expect("pending count"), 0);
        assert_eq!(store.dlq_count().expect("dlq count"), 1);
    }

    #[test]
    fn replay_moves_dlq_back_to_pending() {
        let tmp = TempDir::new().expect("tempdir");
        let store = RelayStore::open(&tmp.path().join("relay.redb")).expect("store");
        let now = 1_700_000_000;

        let event = sample_event(
            "event-1",
            "github:d1:opened:42",
            "cooldown-github-org-repo-42",
            now,
        );
        assert_eq!(
            store
                .enqueue_pending_event(event, 7 * 24 * 60 * 60, 30, now)
                .expect("enqueue"),
            EnqueueResult::Enqueued
        );

        let popped = store.pop_due_event(now).expect("pop due").expect("event");
        store
            .move_to_dlq(popped, "forward_failed", now + 2)
            .expect("move to dlq");

        assert!(store.replay_dlq_event("event-1", now + 5).expect("replay"));
        assert_eq!(store.dlq_count().expect("dlq count"), 0);
        assert_eq!(store.pending_count().expect("pending count"), 1);
    }
}
