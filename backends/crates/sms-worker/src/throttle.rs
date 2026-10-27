#![doc = include_str!("throttle.md")]

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Admits a key at most once per `every`.
pub(crate) struct Throttle {
    every: Duration,
    last_admitted: Mutex<HashMap<String, Instant>>,
}

impl Throttle {
    pub(crate) fn new(every: Duration) -> Self {
        Self {
            every,
            last_admitted: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `key` may be reported at `now`: true the first time, and again
    /// only once `every` has passed since it last was.
    pub(crate) fn admit(&self, key: &str, now: Instant) -> bool {
        let mut last = self
            .last_admitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match last.get(key) {
            Some(admitted) if now.saturating_duration_since(*admitted) < self.every => false,
            _ => {
                last.insert(key.to_owned(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_admitted_once_per_interval_and_keys_are_independent() {
        let start = Instant::now();
        let throttle = Throttle::new(Duration::from_secs(60));

        assert!(throttle.admit("a", start));
        assert!(!throttle.admit("a", start));
        assert!(!throttle.admit("a", start + Duration::from_secs(59)));
        assert!(
            throttle.admit("b", start + Duration::from_secs(1)),
            "another key has its own clock"
        );
        assert!(throttle.admit("a", start + Duration::from_secs(60)));
        assert!(
            !throttle.admit("a", start + Duration::from_secs(61)),
            "and the interval restarts from the last admission"
        );
    }
}
