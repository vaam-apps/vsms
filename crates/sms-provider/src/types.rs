use chrono::{DateTime, Utc};
use sms_encoding::SmsEncoding;

/// One message to submit. E.164 throughout — normalisation already happened
/// upstream (§3.2 step 3); an adapter should never need to guess a country
/// code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitRequest {
    /// Destination, E.164.
    pub to: String,
    /// Approved sender ID (numeric, alphanumeric, or short code) to submit
    /// under.
    pub sender_id: String,
    /// The body to send, already normalised (§2.2's typographic-only pass)
    /// and, if the caller opted in, transliterated.
    pub body: String,
    /// What the body was classified as. An adapter without
    /// [`crate::Capabilities::ucs2`] should reject a `Ucs2` request rather
    /// than submit and mangle it.
    pub encoding: SmsEncoding,
    /// Our own idempotency handle for this submission — a `Message.id` or
    /// equivalent — for providers whose API supports client-supplied
    /// dedupe. Not a guarantee every provider honours it, but the one
    /// stable string an adapter always has to offer.
    pub reference: String,
}

/// What a provider handed back for a successful [`crate::SmsProvider::submit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitAck {
    /// The provider's own reference, used to correlate a later DLR back to
    /// this submission. Stored as `Message.providerMessageRef`.
    pub provider_ref: String,
    /// A second form of the same reference, when a provider reports it two
    /// different ways in two different places — the SMPP trap named in
    /// §6.2: `submit_sm_resp` in hex, `deliver_sm`'s receipt in decimal.
    /// Stored as `Message.providerMessageRefAlt`, so DLR correlation can try
    /// both without the schema caring which one the provider chose. `None`
    /// for adapters (Orange included) with one canonical reference form.
    pub provider_ref_alt: Option<String>,
}

/// A DLR (or DLR-shaped provider callback) exactly as the provider sent it,
/// before [`crate::SmsProvider::parse_dlr`] makes sense of it. Kept as raw
/// bytes plus headers rather than a parsed body — signature verification
/// (where a provider supports it) needs the exact bytes, not a
/// re-serialisation of whatever a JSON parser decided the payload meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCallback {
    /// Request headers, in the order received.
    pub headers: Vec<(String, String)>,
    /// The raw request body.
    pub body: Vec<u8>,
}

/// What actually happened to a message, canonicalised across providers.
///
/// Deliberately not [`crate::SmsProvider`]-specific and not the schema's
/// `DeliveryOutcome` directly — this crate has no dependency on `cratestack`
/// or the generated schema (matching `sms-encoding`/`sms-msisdn`'s shape),
/// so it owns a pure equivalent; `sms-api` maps between the two the same
/// way it already maps [`SmsEncoding`] onto the schema's `Encoding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryOutcome {
    /// Confirmed delivered to the handset.
    Delivered,
    /// The provider reported neither success nor a definite failure within
    /// its own window — not retried automatically; see §7.4's `uncertain`
    /// state and its 6-hour timer.
    Uncertain,
    /// Confirmed failed, retryable per the message's own backoff schedule.
    Failed,
    /// The provider's own validity window elapsed with no confirmation
    /// either way.
    Expired,
    /// The provider refused the message outright (unroutable destination,
    /// blocked sender, etc.) — not retryable.
    Rejected,
    /// A status the adapter could not classify into any of the above.
    /// Never silently mapped to `Failed` or `Delivered` — an operator
    /// reading `rawStatus` should be able to trust that `Unknown` really
    /// means "the adapter didn't recognise this," not "the adapter guessed."
    Unknown,
}

/// One canonicalised delivery update, from a push DLR or a
/// [`crate::SmsProvider::poll_status`] result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryUpdate {
    /// Which submission this update is about — matched against
    /// `Message.providerMessageRef` (or `providerMessageRefAlt`).
    pub provider_ref: String,
    /// What actually happened, canonicalised.
    pub outcome: DeliveryOutcome,
    /// When the provider says the event happened, if it says. Distinct from
    /// when the DLR arrived (`DeliveryReceipt.receivedAt`, which the
    /// database stamps).
    pub occurred_at: Option<DateTime<Utc>>,
    /// The provider's own status text or code, verbatim, for
    /// `DeliveryReceipt.rawStatus` — kept even when `outcome` is confident,
    /// because an operator debugging a network-specific failure pattern
    /// needs the provider's actual words, not just our classification of
    /// them.
    pub raw_status: String,
    /// A provider-specific error code, when the outcome is a failure kind
    /// and the provider gave one.
    pub error_code: Option<String>,
    /// The network that actually delivered (or attempted) the message,
    /// when the provider's own DLR reports one — `"mtn"`/`"orange"`/
    /// `"camtel"`/`"nexttel"`, the same lowercase-verbatim vocabulary
    /// `OperatorPrefixRule.operator` and every operator-coded schema enum
    /// already use, not a typed enum: this crate stays framework-free (no
    /// dependency on the schema), and a raw string matching the wire
    /// vocabulary is the same choice `sms-msisdn`'s own
    /// `OperatorPrefixTable` already made for the identical reason.
    ///
    /// `None` when the provider's DLR shape doesn't carry this — most
    /// providers don't. §7's own reasoning for wanting it at all: prefix
    /// routing is a hint, never load-bearing, and "record the delivering
    /// network where the DLR reports it" is what lets observed data
    /// correct `OperatorPrefixRule` over time. A `None` here must fall
    /// back to whatever the message's own prefix-based classification
    /// already recorded, not to a guess this crate makes up.
    pub delivering_network: Option<String>,
}

/// The result of [`crate::SmsProvider::health`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    /// Whether the provider is usable right now.
    pub healthy: bool,
    /// When this check ran — always `Utc::now()` at the call site, but
    /// carried explicitly rather than implied, so a cached/stale health
    /// result is visible as such to whoever reads it.
    pub checked_at: DateTime<Utc>,
    /// Why, when `healthy` is `false` — the provider's own error text, not
    /// a generic "unhealthy".
    pub detail: Option<String>,
}

impl Health {
    /// A healthy result, timestamped now.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            healthy: true,
            checked_at: Utc::now(),
            detail: None,
        }
    }

    /// An unhealthy result, timestamped now, naming why.
    #[must_use]
    pub fn unhealthy(detail: impl Into<String>) -> Self {
        Self {
            healthy: false,
            checked_at: Utc::now(),
            detail: Some(detail.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Health;

    #[test]
    fn ok_is_healthy_with_no_detail() {
        let health = Health::ok();
        assert!(health.healthy);
        assert!(health.detail.is_none());
    }

    #[test]
    fn unhealthy_carries_why() {
        let health = Health::unhealthy("token refresh failed");
        assert!(!health.healthy);
        assert_eq!(health.detail.as_deref(), Some("token refresh failed"));
    }
}
