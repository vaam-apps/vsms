#![doc = include_str!("ledger.md")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fault::SubmitOutcome;

/// One received submit call, as the fake saw it.
#[derive(Debug, Clone)]
pub struct SubmitRecord {
    /// The destination this submit call named, from the wire's own
    /// documented `address` field (`tel:` scheme stripped so it matches
    /// `Message.msisdn` verbatim). Orange's real, documented submit
    /// request (<https://developer.orange.com/apis/sms/getting-started>)
    /// carries no caller-supplied reference of any kind — no
    /// `Message.id`, no idempotency token, nothing — so `to` is the
    /// closest thing this fake (or real Orange) can offer for "which
    /// logical message was this submit call for." A caller wanting to
    /// correlate multiple submit calls (e.g. retries) as the same message
    /// has to seed distinct recipients per concurrent message and group
    /// on this field — see [`Ledger::submit_count`].
    pub to: String,
    /// The `resource_id` this fake minted for this call and echoed back
    /// both in the `201`'s `resourceURL` and, if a DLR was scheduled, in
    /// the DLR's own `callbackData` — mirroring how real Orange mints and
    /// reports one id per submission (§4 "About SMS Delivery Receipt"),
    /// never a caller-supplied one. A fresh id every call, even for two
    /// calls a caller intended as retries of the same logical message:
    /// real Orange has no way to recognise a retry as "the same"
    /// submission either, which is exactly the accepted, documented
    /// consequence `sms-provider-orange-cm`'s own `lib.rs` records for an
    /// `Indeterminate` outcome.
    pub resource_id: String,
    /// Which [`SubmitOutcome`] the fault policy chose for this call.
    pub outcome: SubmitOutcome,
    /// The response delay the fault policy chose for this call — a caller
    /// comparing this against its own configured `request_timeout` can tell
    /// whether a given submit call would have read as `Indeterminate` on
    /// the client side, without needing to duplicate that classification
    /// logic here (this crate has no opinion on any caller's timeout).
    pub response_delay: Duration,
    /// When the fake received this request.
    pub at: Instant,
}

/// Every submit call received, plus in-flight DLR bookkeeping. Cheap to
/// clone (an `Arc` internally) — held by the fake's own responder and handed
/// out to test code via [`crate::FakeOrange::ledger`].
#[derive(Debug, Default)]
pub struct Ledger {
    submits: Mutex<Vec<SubmitRecord>>,
    pending_dlrs: AtomicUsize,
}

impl Ledger {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn record_submit(
        &self,
        to: &str,
        resource_id: &str,
        outcome: &SubmitOutcome,
        response_delay: Duration,
    ) {
        self.submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(SubmitRecord {
                to: to.to_owned(),
                resource_id: resource_id.to_owned(),
                outcome: outcome.clone(),
                response_delay,
                at: Instant::now(),
            });
    }

    pub(crate) fn mark_dlr_pending(&self) {
        self.pending_dlrs.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn mark_dlr_settled(&self) {
        self.pending_dlrs.fetch_sub(1, Ordering::SeqCst);
    }

    /// A snapshot of every submit call received so far, oldest first.
    #[must_use]
    pub fn submits(&self) -> Vec<SubmitRecord> {
        self.submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many submit calls this fake received naming `to` as the
    /// destination address (`Message.msisdn`, no `tel:` scheme). This
    /// replaces counting by a caller-supplied reference — real Orange's
    /// documented submit request has none — so a caller retrying the same
    /// logical message resubmits the same recipient every time, and a
    /// test wanting to count retries of one message among several
    /// concurrent ones must seed each with a distinct recipient (see
    /// [`SubmitRecord::to`]'s own doc).
    #[must_use]
    pub fn submit_count(&self, to: &str) -> usize {
        self.submits().iter().filter(|r| r.to == to).count()
    }

    /// How many scheduled DLR-delivery tasks have not yet completed their
    /// HTTP round trip.
    #[must_use]
    pub fn pending_dlrs(&self) -> usize {
        self.pending_dlrs.load(Ordering::SeqCst)
    }

    /// Polls [`Self::pending_dlrs`] down to zero, or gives up after
    /// `timeout` and returns `false`. Prefer this over a fixed sleep: the
    /// fake's own DLR delays are chosen per fault policy, not known to the
    /// caller, and a fixed sleep either wastes time or races a slow one.
    pub async fn wait_for_dlrs_to_settle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.pending_dlrs() > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        true
    }
}
