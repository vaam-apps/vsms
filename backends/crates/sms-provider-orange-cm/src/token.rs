#![doc = include_str!("token.md")]

use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::Deserialize;
use sms_provider::ProviderError;

/// A cached bearer token, valid until the instant recorded — already backed
/// off from the token's real expiry (see [`TokenCache::store`]).
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    valid_until: Instant,
}

/// Refresh at this fraction of the token's stated `expires_in`, per §6.2.
/// `0.8` rather than exactly `1.0`: refreshing on a fixed early margin
/// means every token still has room to serve requests already in flight
/// when the *next* refresh kicks off, rather than a request racing the
/// token's hard expiry.
const REFRESH_AT_FRACTION_OF_LIFETIME: f64 = 0.8;

/// Caches one bearer token, refreshing it only once its 80%-of-`expires_in`
/// margin has passed.
///
/// A plain [`std::sync::RwLock`], not `tokio::sync::RwLock` — every
/// critical section here is a pointer read/write with no `.await` inside
/// it, so there is nothing async to block on and no reason to pay for an
/// async-aware lock.
pub(crate) struct TokenCache {
    cached: RwLock<Option<CachedToken>>,
}

impl TokenCache {
    pub(crate) fn new() -> Self {
        Self {
            cached: RwLock::new(None),
        }
    }

    /// The currently cached token, if it hasn't passed its refresh margin.
    pub(crate) fn valid(&self) -> Option<String> {
        let cached = self
            .cached
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cached
            .as_ref()
            .filter(|token| Instant::now() < token.valid_until)
            .map(|token| token.access_token.clone())
    }

    /// Cache a freshly fetched token, computing its refresh margin from the
    /// server-reported lifetime.
    pub(crate) fn store(&self, access_token: String, expires_in: Duration) {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let backed_off =
            Duration::from_secs_f64(expires_in.as_secs_f64() * REFRESH_AT_FRACTION_OF_LIFETIME);
        let mut cached = self
            .cached
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *cached = Some(CachedToken {
            access_token,
            valid_until: Instant::now() + backed_off,
        });
    }
}

/// `POST /oauth/v3/token` response — only the fields this crate reads.
#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) access_token: String,
    /// Seconds, per `OAuth2` (RFC 6749 §5.1) — §6.2 states Orange's is 3600.
    pub(crate) expires_in: u64,
}

/// Fetch a fresh token via `client_credentials` over HTTP Basic auth — the
/// standard `OAuth2` shape (RFC 6749 §4.4, §2.3.1), which is also what §6.2
/// specifies for this endpoint.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<TokenResponse, ProviderError> {
    let response = client
        .post(token_url)
        .basic_auth(client_id, Some(client_secret))
        .form(&[("grant_type", "client_credentials")])
        .send()
        .await
        .map_err(|error| ProviderError::Unavailable {
            message: format!("token request failed: {error}"),
            source: Some(Box::new(error)),
        })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(if status.is_server_error() {
            ProviderError::Unavailable {
                message: format!("token endpoint returned {status}: {body}"),
                // A real response was received and read as text — there is
                // no leftover `reqwest::Error`/parse failure to chain here.
                source: None,
            }
        } else {
            // A 4xx acquiring a token is almost always bad credentials, not
            // a per-message problem — but there is no per-message routing
            // decision to make about it either, since without a token
            // *nothing* on this provider can be attempted. Permanent (try a
            // different provider) rather than Rejected (fail the message):
            // the message itself may well be perfectly sendable elsewhere.
            ProviderError::Permanent {
                code: format!("oauth_{}", status.as_u16()),
                message: format!("token endpoint rejected credentials: {body}"),
            }
        });
    }

    response
        .json()
        .await
        .map_err(|error| ProviderError::Unavailable {
            message: format!("token response was not valid JSON: {error}"),
            source: Some(Box::new(error)),
        })
}

#[cfg(test)]
mod tests {
    use super::TokenCache;
    use std::time::Duration;

    #[test]
    fn a_fresh_cache_has_no_valid_token() {
        assert!(TokenCache::new().valid().is_none());
    }

    #[test]
    fn a_just_stored_token_is_valid() {
        let cache = TokenCache::new();
        cache.store("tok-1".to_owned(), Duration::from_hours(1));
        assert_eq!(cache.valid().as_deref(), Some("tok-1"));
    }

    #[test]
    fn a_token_past_its_80_percent_margin_is_not_valid() {
        let cache = TokenCache::new();
        // A token whose entire lifetime has already elapsed is well past
        // its 80% mark regardless of the exact fraction — this is the
        // unambiguous end of the test, not a boundary case.
        cache.store("tok-1".to_owned(), Duration::from_secs(0));
        assert!(cache.valid().is_none());
    }

    #[test]
    fn storing_a_new_token_replaces_the_old_one() {
        let cache = TokenCache::new();
        cache.store("tok-1".to_owned(), Duration::from_hours(1));
        cache.store("tok-2".to_owned(), Duration::from_hours(1));
        assert_eq!(cache.valid().as_deref(), Some("tok-2"));
    }
}
