#![doc = include_str!("transport.md")]

use sms_provider::ProviderError;

/// See this module's own doc for the full reasoning behind the predicate
/// order below — this comment covers only the one thing that's new versus
/// the two adapter-local copies this function replaces: `provider`.
///
/// `provider` fills the single place this classification was never fully
/// provider-agnostic: the [`ProviderError::Indeterminate`] message names
/// who "may have received it". Every caller passes its own noun exactly as
/// it appeared in that crate's pre-consolidation message text — Orange
/// passed `"Orange"`, MTN passes `"the aggregator"` — so the text this
/// function produces is byte-identical to what each adapter said before
/// this crate existed. The other two branches never mention a provider at
/// all in either adapter's original wording, so `provider` is simply
/// unused there; it stays a plain `&str` parameter (not, say, an
/// `Option<&str>` used everywhere) because every real caller has a noun to
/// give, and requiring one is cheaper than every caller re-deciding
/// whether it has one.
#[must_use]
pub fn classify_transport_error(error: reqwest::Error, provider: &str) -> ProviderError {
    if error.is_connect() {
        let message = format!("submit request failed to connect: {error}");
        return ProviderError::Unavailable {
            message,
            source: Some(Box::new(error)),
        };
    }
    if error.is_timeout() || error.is_body() {
        let message = format!(
            "submit request timed out or was interrupted after the connection was \
             already established; {provider} may have received it: {error}"
        );
        return ProviderError::Indeterminate {
            message,
            source: Some(Box::new(error)),
        };
    }
    let message = format!("submit request failed: {error}");
    ProviderError::Unavailable {
        message,
        source: Some(Box::new(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_transport_error;
    use sms_provider::ProviderError;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Proves the safe branch directly, with a real connection-refused
    /// error rather than a mocked one: bind an ephemeral port, then drop
    /// the listener so the address is valid but refuses every connection.
    /// Nothing about a submission could have reached whatever might
    /// eventually listen there, so this must stay exactly as safe to retry
    /// as it always was. Identical setup to the two adapter crates' own
    /// `a_connect_refusal_is_still_unavailable` — this is the shared
    /// function's own copy of the same proof, not a replacement for
    /// theirs; see AGENTS.md on why both levels are kept.
    #[tokio::test]
    async fn a_connect_refusal_is_still_unavailable() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
        let addr = listener.local_addr().expect("reading the bound address");
        drop(listener);

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("building a plain reqwest client");
        let error = client
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect_err("nothing listens on a dropped ephemeral port");

        assert!(
            error.is_connect(),
            "test setup: expected a connect-level failure, got {error:?}"
        );
        let classified = classify_transport_error(error, "test-provider");
        assert!(matches!(classified, ProviderError::Unavailable { .. }));
        assert!(
            std::error::Error::source(&classified).is_some(),
            "a real reqwest::Error occurred; it must be recoverable via source()"
        );
    }

    /// Proves the unsafe branch directly: a connection that *does*
    /// establish, against a server that then never answers before the
    /// client's own timeout fires. This is exactly the shape a slow/hung
    /// provider endpoint produces — the request may already be sitting on
    /// the provider's side. Identical setup to the two adapter crates' own
    /// `a_post_connect_timeout_is_indeterminate`.
    #[tokio::test]
    async fn a_post_connect_timeout_is_indeterminate() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_millis(50))
            .build()
            .expect("building a plain reqwest client");
        let error = client
            .get(format!("{}/slow", server.uri()))
            .send()
            .await
            .expect_err("the mock's delay exceeds the client's own timeout");

        assert!(
            error.is_timeout(),
            "test setup: expected a timeout, got {error:?}"
        );
        assert!(
            !error.is_connect(),
            "test setup: the connection must already be established when the timeout fires, \
             or this isn't testing the branch it claims to"
        );
        let classified = classify_transport_error(error, "the aggregator");
        match classified {
            ProviderError::Indeterminate { message, source } => {
                assert!(
                    message.contains("the aggregator may have received it"),
                    "the provider noun must reach the message text verbatim: {message}"
                );
                assert!(
                    source.is_some(),
                    "a real reqwest::Error occurred; it must be recoverable via source()"
                );
            }
            other => panic!("expected Indeterminate, got {other:?}"),
        }
    }

    /// The one thing every caller relies on and nothing above proves on
    /// its own: swapping the two live branches must actually break both of
    /// them, not just one. See AGENTS.md's own "Cleanup: one transport
    /// classifier for every HTTP adapter" section for the sabotage-and-
    /// restore run this test's own shape is verified against — this test
    /// doesn't perform the sabotage itself (that's done by hand, once, and
    /// reverted), it's the fixed point the sabotage is checked against.
    #[tokio::test]
    async fn connect_and_post_connect_failures_classify_to_different_variants() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port");
        let addr = listener.local_addr().expect("reading the bound address");
        drop(listener);
        let refusing_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .expect("building a plain reqwest client");
        let connect_error = refusing_client
            .get(format!("http://{addr}"))
            .send()
            .await
            .expect_err("nothing listens on a dropped ephemeral port");

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;
        let timing_out_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_millis(50))
            .build()
            .expect("building a plain reqwest client");
        let timeout_error = timing_out_client
            .get(format!("{}/slow", server.uri()))
            .send()
            .await
            .expect_err("the mock's delay exceeds the client's own timeout");

        assert!(matches!(
            classify_transport_error(connect_error, "x"),
            ProviderError::Unavailable { .. }
        ));
        assert!(matches!(
            classify_transport_error(timeout_error, "x"),
            ProviderError::Indeterminate { .. }
        ));
    }
}
