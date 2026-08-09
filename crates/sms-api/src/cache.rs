//! A time-to-live cache, used twice on `sendMessage`'s hot path: the
//! `clientId → App` lookup (§3.2: *"the token carries no `appId`... that's
//! on the hot path, so cache it — 60 seconds is short enough that retiring
//! a client takes effect promptly and long enough that the lookup never
//! matters"*) and the operator-prefix table (AGENTS.md: `previewMessage`
//! has reported `operator: unknown` since milestone 0 because nothing
//! queried `OperatorPrefixRule` yet — *"querying the DB means giving it a
//! cache and a refresh policy"*).
//!
//! **Not** the `LISTEN`/`NOTIFY`-based opt-out-invalidation cache §11's
//! repository layout names this file for. That's a real R1 exception
//! (`LISTEN` is one of the three named ones) and a more sophisticated
//! mechanism than either cache here needs — a 60-second staleness window on
//! an `App` lookup is the behaviour §3.2 explicitly asks for, not a gap to
//! close. If a `LISTEN`-driven cache is built later, for opt-outs or
//! anything else, it can live in this same file; this type doesn't preclude
//! it.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Caches `V` by `K`, re-fetching once an entry is older than `ttl`.
///
/// A plain [`std::sync::RwLock`], not `tokio::sync::RwLock`: every critical
/// section is a `HashMap` read or write with no `.await` inside it, so
/// there is nothing async to block on. Fetching a *missing* or *stale*
/// value is the caller's job — see [`TtlCache::get_or_fetch`] — because
/// only the caller knows how, and a cache has no business embedding a
/// database query.
pub(crate) struct TtlCache<K, V> {
    ttl: Duration,
    entries: RwLock<HashMap<K, (V, Instant)>>,
}

impl<K, V> TtlCache<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    pub(crate) fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// The cached value for `key`, if present and not yet stale.
    fn get(&self, key: &K) -> Option<V> {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (value, fetched_at) = entries.get(key)?;
        (fetched_at.elapsed() < self.ttl).then(|| value.clone())
    }

    /// The cached value for `key`, or the result of `fetch(key)` on a miss
    /// or a stale entry — cached in turn, so the next call within `ttl`
    /// doesn't pay for `fetch` again. `fetch` receives the key by value
    /// (not a reference) so it can be moved into an `async move` block
    /// without a lifetime tying it back to this call.
    ///
    /// `fetch` is only ever called while holding no lock — an `Err` from it
    /// propagates straight out, leaving the cache exactly as it was, so a
    /// transient database error never poisons a future lookup with a
    /// negative result.
    ///
    /// No single-flight coordination on a miss: N concurrent callers racing
    /// the same stale/absent key each run `fetch` and each write their own
    /// result (last write wins, all of them correct). Flagged in review
    /// (#94) as a thundering-herd risk on `operator_cache`, whose key is
    /// `()` — every concurrent `sendMessage` call sees the same TTL expiry
    /// at once. Accepted rather than fixed: a burst of duplicate reads
    /// against a 14-row table on a 5-minute TTL costs nothing worth a
    /// `tokio::sync::OnceCell`-style in-flight gate for. Revisit if a
    /// future cached query is expensive enough that N-at-once matters.
    pub(crate) async fn get_or_fetch<F, Fut, E>(&self, key: K, fetch: F) -> Result<V, E>
    where
        F: FnOnce(K) -> Fut,
        Fut: std::future::Future<Output = Result<V, E>>,
    {
        if let Some(value) = self.get(&key) {
            return Ok(value);
        }

        let value = fetch(key.clone()).await?;

        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(key, (value.clone(), Instant::now()));
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::TtlCache;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn a_miss_fetches_and_caches() {
        let cache: TtlCache<&str, i32> = TtlCache::new(Duration::from_mins(1));
        let calls = AtomicUsize::new(0);

        let first: Result<i32, std::convert::Infallible> = cache
            .get_or_fetch("k", |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await;
        let second: Result<i32, std::convert::Infallible> = cache
            .get_or_fetch("k", |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            })
            .await;

        assert_eq!(first.unwrap(), 42);
        assert_eq!(second.unwrap(), 42);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the second call must be served from cache, not fetch again"
        );
    }

    #[tokio::test]
    async fn a_stale_entry_is_refetched() {
        let cache: TtlCache<&str, i32> = TtlCache::new(Duration::from_millis(1));
        let calls = AtomicUsize::new(0);
        let fetch = |_: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            async {
                Ok::<_, std::convert::Infallible>(
                    i32::try_from(calls.load(Ordering::SeqCst)).unwrap(),
                )
            }
        };

        cache.get_or_fetch("k", fetch).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let second = cache.get_or_fetch("k", fetch).await.unwrap();

        assert_eq!(second, 2, "a stale entry must be refetched, not reused");
    }

    #[tokio::test]
    async fn a_failed_fetch_does_not_poison_the_cache() {
        let cache: TtlCache<&str, i32> = TtlCache::new(Duration::from_mins(1));

        let failed = cache
            .get_or_fetch("k", |_| async { Err::<i32, _>("boom") })
            .await;
        assert_eq!(failed, Err("boom"));

        let recovered = cache
            .get_or_fetch("k", |_| async { Ok::<_, &str>(7) })
            .await;
        assert_eq!(
            recovered,
            Ok(7),
            "a failed fetch must not cache a negative result"
        );
    }

    #[tokio::test]
    async fn different_keys_are_independent() {
        let cache: TtlCache<&str, i32> = TtlCache::new(Duration::from_mins(1));
        cache
            .get_or_fetch("a", |_| async { Ok::<_, std::convert::Infallible>(1) })
            .await
            .unwrap();
        cache
            .get_or_fetch("b", |_| async { Ok::<_, std::convert::Infallible>(2) })
            .await
            .unwrap();

        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get(&"b"), Some(2));
    }
}
