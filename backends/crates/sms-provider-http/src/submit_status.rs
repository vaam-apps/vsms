#![doc = include_str!("submit_status.md")]

use std::time::Duration;

use reqwest::StatusCode;
use sms_provider::ProviderError;

/// Classifies a well-formed non-`2xx` response — see this module's own doc
/// for exactly which statuses this covers, which are left to each
/// adapter's own `classify_submit_error`, and why. `rate_limit_retry_after`
/// is the one place the two adapters' own numbers genuinely differ (a
/// negotiated commercial fact, not something this function should invent
/// on either's behalf); `provider` is the noun each adapter's own `5xx`
/// message already named before this function existed (`"orange"` /
/// `"aggregator"`, preserved verbatim).
#[must_use]
pub fn classify_common_submit_status(
    status: StatusCode,
    body: &str,
    provider: &str,
    rate_limit_retry_after: Duration,
) -> ProviderError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ProviderError::Transient {
            retry_after: rate_limit_retry_after,
            message: format!("rate limited: {body}"),
        };
    }
    if status.is_server_error() {
        return ProviderError::Unavailable {
            message: format!("{provider} returned {status}: {body}"),
            source: None,
        };
    }
    ProviderError::Rejected {
        code: format!("http_{}", status.as_u16()),
        message: body.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_common_submit_status;
    use reqwest::StatusCode;
    use sms_provider::ProviderError;
    use std::time::Duration;

    #[test]
    fn rate_limited_is_transient_with_the_callers_own_retry_after() {
        let error = classify_common_submit_status(
            StatusCode::TOO_MANY_REQUESTS,
            "slow down",
            "orange",
            Duration::from_secs(1),
        );
        match error {
            ProviderError::Transient {
                retry_after,
                message,
            } => {
                assert_eq!(retry_after, Duration::from_secs(1));
                assert_eq!(message, "rate limited: slow down");
            }
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn a_server_error_is_unavailable_with_the_callers_own_provider_noun() {
        let error = classify_common_submit_status(
            StatusCode::SERVICE_UNAVAILABLE,
            "down for maintenance",
            "aggregator",
            Duration::from_secs(5),
        );
        match error {
            ProviderError::Unavailable { message, source } => {
                assert_eq!(
                    message,
                    "aggregator returned 503 Service Unavailable: down for maintenance"
                );
                assert!(
                    source.is_none(),
                    "no real error object exists at this layer"
                );
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn anything_else_is_rejected_with_the_status_as_the_code() {
        let error = classify_common_submit_status(
            StatusCode::BAD_REQUEST,
            "malformed destination",
            "orange",
            Duration::from_secs(1),
        );
        match error {
            ProviderError::Rejected { code, message } => {
                assert_eq!(code, "http_400");
                assert_eq!(message, "malformed destination");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }
}
