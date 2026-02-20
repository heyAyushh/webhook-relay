use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyDecision {
    Accept,
    Duplicate,
    Cooldown,
}

#[derive(Debug, Clone)]
pub struct IdempotencyStore {
    dedup_ttl_seconds: i64,
    cooldown_seconds: i64,
    dedup_expirations: Arc<Mutex<HashMap<String, i64>>>,
    cooldown_expirations: Arc<Mutex<HashMap<String, i64>>>,
}

impl IdempotencyStore {
    pub fn new(dedup_ttl_seconds: i64, cooldown_seconds: i64) -> Self {
        Self {
            dedup_ttl_seconds,
            cooldown_seconds,
            dedup_expirations: Arc::new(Mutex::new(HashMap::new())),
            cooldown_expirations: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn check(
        &self,
        dedup_key: &str,
        cooldown_key: Option<&str>,
        now_epoch: i64,
    ) -> IdempotencyDecision {
        if dedup_key.is_empty() {
            return IdempotencyDecision::Accept;
        }

        {
            let mut dedup_guard = match self.dedup_expirations.lock() {
                Ok(guard) => guard,
                Err(_) => return IdempotencyDecision::Duplicate,
            };

            prune_expired(&mut dedup_guard, now_epoch);
            if let Some(expires_at) = dedup_guard.get(dedup_key)
                && *expires_at > now_epoch
            {
                return IdempotencyDecision::Duplicate;
            }

            dedup_guard.insert(dedup_key.to_string(), now_epoch + self.dedup_ttl_seconds);
        }

        let Some(cooldown_key) = cooldown_key else {
            return IdempotencyDecision::Accept;
        };

        if cooldown_key.is_empty() {
            return IdempotencyDecision::Accept;
        }

        let mut cooldown_guard = match self.cooldown_expirations.lock() {
            Ok(guard) => guard,
            Err(_) => return IdempotencyDecision::Cooldown,
        };

        prune_expired(&mut cooldown_guard, now_epoch);
        if let Some(expires_at) = cooldown_guard.get(cooldown_key)
            && *expires_at > now_epoch
        {
            return IdempotencyDecision::Cooldown;
        }

        cooldown_guard.insert(cooldown_key.to_string(), now_epoch + self.cooldown_seconds);
        IdempotencyDecision::Accept
    }
}

fn prune_expired(cache: &mut HashMap<String, i64>, now_epoch: i64) {
    cache.retain(|_, expires_at| *expires_at > now_epoch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_key_is_rejected_within_ttl() {
        let store = IdempotencyStore::new(60, 30);
        assert_eq!(
            store.check("dedup-1", Some("cooldown-1"), 1_700_000_000),
            IdempotencyDecision::Accept
        );
        assert_eq!(
            store.check("dedup-1", Some("cooldown-1"), 1_700_000_010),
            IdempotencyDecision::Duplicate
        );
    }

    #[test]
    fn cooldown_rejects_new_delivery_for_same_entity() {
        let store = IdempotencyStore::new(600, 30);
        assert_eq!(
            store.check("dedup-1", Some("cooldown-1"), 1_700_000_000),
            IdempotencyDecision::Accept
        );
        assert_eq!(
            store.check("dedup-2", Some("cooldown-1"), 1_700_000_010),
            IdempotencyDecision::Cooldown
        );
    }

    #[test]
    fn keys_expire_and_accept_again() {
        let store = IdempotencyStore::new(60, 30);
        assert_eq!(
            store.check("dedup-1", Some("cooldown-1"), 1_700_000_000),
            IdempotencyDecision::Accept
        );
        assert_eq!(
            store.check("dedup-2", Some("cooldown-1"), 1_700_000_031),
            IdempotencyDecision::Accept
        );
        assert_eq!(
            store.check("dedup-1", Some("cooldown-2"), 1_700_000_061),
            IdempotencyDecision::Accept
        );
    }
}
