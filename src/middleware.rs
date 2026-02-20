use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SECONDS_PER_MINUTE: i64 = 60;

#[derive(Debug, Clone, Copy)]
struct SourceRateWindow {
    minute_bucket: i64,
    count: u32,
}

#[derive(Debug, Clone)]
pub struct SourceRateLimiter {
    limit_per_minute: u32,
    windows: Arc<Mutex<HashMap<String, SourceRateWindow>>>,
}

impl SourceRateLimiter {
    pub fn new(limit_per_minute: u32) -> Self {
        Self {
            limit_per_minute,
            windows: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn allow(&self, source: &str, now_epoch: i64) -> bool {
        let now_minute = now_epoch / SECONDS_PER_MINUTE;
        let mut guard = match self.windows.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };

        let entry = guard.entry(source.to_string()).or_insert(SourceRateWindow {
            minute_bucket: now_minute,
            count: 0,
        });

        if entry.minute_bucket != now_minute {
            entry.minute_bucket = now_minute;
            entry.count = 0;
        }

        if entry.count >= self.limit_per_minute {
            return false;
        }

        entry.count = entry.count.saturating_add(1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_limiter_resets_each_minute() {
        let limiter = SourceRateLimiter::new(2);

        assert!(limiter.allow("github", 60));
        assert!(limiter.allow("github", 60));
        assert!(!limiter.allow("github", 60));

        assert!(limiter.allow("github", 120));
    }
}
