//! The request ledger — every submit [`FakeOrange`](crate::FakeOrange)
//! received, queryable by test code, plus a counter of DLR-delivery tasks
//! still in flight so a test can wait for the fake's own background work to
//! settle instead of guessing a sleep duration.
//!
//! This is how a chaos test detects double-submission *from the provider's
//! own side* — "did Orange receive this reference twice?" — rather than
//! only inferring it from our database's own `attempts` column, which
//! cannot distinguish "one HTTP call that was retried at the transport
//! level" from "two logically separate submit attempts".

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::fault::SubmitOutcome;

/// One received submit call, as the fake saw it.
#[derive(Debug, Clone)]
pub struct SubmitRecord {
    /// `receiptRequest.callbackData` from the request — `Message.id` on
    /// every real call this fake ever receives, per
    /// `OrangeCmProvider::submit`'s own contract. Empty string if the
    /// request body couldn't be parsed at all (a test bug, not a fault this
    /// crate injects — nothing in [`crate::fault`] ever omits `callbackData`).
    pub reference: String,
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
        reference: &str,
        outcome: &SubmitOutcome,
        response_delay: Duration,
    ) {
        self.submits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(SubmitRecord {
                reference: reference.to_owned(),
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

    /// How many times a given reference (`Message.id`) was submitted.
    #[must_use]
    pub fn submit_count(&self, reference: &str) -> usize {
        self.submits()
            .iter()
            .filter(|r| r.reference == reference)
            .count()
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
