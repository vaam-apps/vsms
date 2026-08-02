//! The abstraction every SMS provider adapter — HTTP or SMPP — fits behind.
//! §6.1 of the design doc.
//!
//! Pure, like `sms-encoding` and `sms-msisdn`: no `cratestack` dependency,
//! no schema types. [`SmsProvider`] is what `sms-worker`'s `dispatch` role
//! (#33) will call through; concrete adapters (`sms-provider-orange-cm`,
//! and later MTN, an aggregator, SMPP) are separate crates that implement
//! it. Nothing here decides *which* provider gets a message — that's
//! routing (§6.3), a `sms-worker` concern, not this crate's.
//!
//! Two things carry the whole design, and both are types, not prose a
//! caller has to remember:
//!
//! - [`Capabilities`] — what a provider can do, so routing asks
//!   `capabilities.ucs2` instead of special-casing a provider's identity.
//! - [`ProviderError`] and [`error::RoutingConsequence`] — what went wrong,
//!   mapped to exactly one routing decision by a compiler-checked match
//!   rather than a comment. See the module doc on [`ProviderError`] for why
//!   this is the part of the whole provider layer most worth getting right.

mod capabilities;
mod error;
mod types;

pub use capabilities::Capabilities;
pub use error::{ProviderError, RoutingConsequence};
pub use types::{DeliveryOutcome, DeliveryUpdate, Health, RawCallback, SubmitAck, SubmitRequest};

use async_trait::async_trait;

/// One provider integration: submit a message, understand its DLRs, report
/// whether it's usable right now.
///
/// `'static` because every implementation is expected to be held behind an
/// `Arc` for the lifetime of the process — a provider is resolved once at
/// worker startup from `Provider.credentialRef` (§2.4: never a secret in
/// the database, always a pointer the worker resolves), not reconstructed
/// per message.
#[async_trait]
pub trait SmsProvider: Send + Sync + 'static {
    /// This provider's `Provider.key` — `"orange_cm"`, not a display name.
    fn key(&self) -> &str;

    /// What this provider can do right now. Not `const` or cached by the
    /// trait itself: an implementation is free to make this reflect live
    /// state (a sender ID approval that lapsed, a feature disabled by
    /// config) rather than a value fixed at construction.
    fn capabilities(&self) -> Capabilities;

    /// Submit one message. `Ok` carries the provider's own reference for
    /// later DLR correlation; `Err` is one of [`ProviderError`]'s four real
    /// variants, each implying exactly one routing decision — see
    /// [`ProviderError::routing`].
    async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError>;

    /// Turn a provider-specific DLR callback into the canonical shape.
    /// `Vec` because some providers batch multiple delivery updates into one
    /// callback.
    fn parse_dlr(&self, raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError>;

    /// Poll for status on providers with no push DLR. Defaults to
    /// [`ProviderError::Unsupported`] — most providers push; only override
    /// this where polling is actually the mechanism.
    async fn poll_status(&self, _refs: &[String]) -> Result<Vec<DeliveryUpdate>, ProviderError> {
        Err(ProviderError::Unsupported)
    }

    /// Whether this provider is usable right now. Feeds the circuit
    /// breaker and route filtering (§6.3) — a provider reporting unhealthy
    /// should be skipped before a message is ever attempted on it, not
    /// discovered unhealthy via a failed `submit`.
    async fn health(&self) -> Health;
}

#[cfg(test)]
mod tests {
    use super::{
        Capabilities, DeliveryOutcome, DeliveryUpdate, Health, ProviderError, RawCallback,
        SmsProvider, SubmitAck, SubmitRequest,
    };
    use async_trait::async_trait;
    use rust_decimal::Decimal;
    use sms_encoding::SmsEncoding;
    use std::sync::Arc;

    /// A minimal, deterministic implementation — proves the trait is
    /// object-safe-in-spirit (usable behind `Arc<dyn SmsProvider>`, which
    /// `dispatch` will need) and that every method is actually callable
    /// with the types this crate exports, without needing a real provider.
    struct EchoProvider;

    #[async_trait]
    impl SmsProvider for EchoProvider {
        // The trait signature is `&self -> &str`; clippy's `'static`
        // suggestion doesn't fit a trait impl that must match it exactly.
        #[allow(clippy::unnecessary_literal_bound)]
        fn key(&self) -> &str {
            "echo"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                dlr: true,
                alphanumeric_sender: true,
                ucs2: true,
                concatenation: true,
                tps_ceiling: 5.0,
                cost_per_segment_xaf: Decimal::new(18, 0),
            }
        }

        async fn submit(&self, req: &SubmitRequest) -> Result<SubmitAck, ProviderError> {
            Ok(SubmitAck {
                provider_ref: format!("echo-{}", req.reference),
                provider_ref_alt: None,
            })
        }

        fn parse_dlr(&self, raw: &RawCallback) -> Result<Vec<DeliveryUpdate>, ProviderError> {
            let provider_ref =
                String::from_utf8(raw.body.clone()).map_err(|_| ProviderError::Rejected {
                    code: "BAD_BODY".to_owned(),
                    message: "not utf-8".to_owned(),
                })?;
            Ok(vec![DeliveryUpdate {
                provider_ref,
                outcome: DeliveryOutcome::Delivered,
                occurred_at: None,
                raw_status: "OK".to_owned(),
                error_code: None,
                delivering_network: None,
            }])
        }

        async fn health(&self) -> Health {
            Health::ok()
        }
    }

    fn provider() -> Arc<dyn SmsProvider> {
        Arc::new(EchoProvider)
    }

    #[tokio::test]
    async fn a_provider_is_usable_behind_arc_dyn() {
        let provider = provider();
        assert_eq!(provider.key(), "echo");
        assert!(provider.capabilities().dlr);
    }

    #[tokio::test]
    async fn submit_round_trips_a_reference() {
        let ack = provider()
            .submit(&SubmitRequest {
                to: "+237677123456".to_owned(),
                sender_id: "VYMALO".to_owned(),
                body: "hello".to_owned(),
                encoding: SmsEncoding::Gsm7,
                reference: "msg-1".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(ack.provider_ref, "echo-msg-1");
    }

    #[tokio::test]
    async fn poll_status_defaults_to_unsupported() {
        let error = provider().poll_status(&[]).await.unwrap_err();
        assert!(matches!(error, ProviderError::Unsupported));
    }

    #[tokio::test]
    async fn health_reports_ok() {
        assert!(provider().health().await.healthy);
    }
}
