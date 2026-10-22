#![doc = include_str!("ledger.md")]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};

/// One `SendTextMessage` request, as the fake received it.
#[derive(Debug, Clone, Default)]
pub struct SubmitRecord {
    /// When it arrived.
    pub received_at: DateTime<Utc>,
    /// `DestinationPhoneNumber`; empty when the request did not parse.
    pub destination: String,
    /// `MessageBody`.
    pub body: String,
    /// `OriginationIdentity`, when sent.
    pub origination_identity: Option<String>,
    /// `MessageType`, when sent.
    pub message_type: Option<String>,
    /// `ConfigurationSetName`, when sent.
    pub configuration_set: Option<String>,
    /// `TimeToLive`, when sent.
    pub time_to_live: Option<u64>,
    /// `Context`.
    pub context: BTreeMap<String, String>,
    /// The `MessageId` the fake assigned, when it accepted the message
    /// (also set for `without_message_id`, which accepts but does not say).
    pub message_id: Option<String>,
    /// The HTTP status answered.
    pub status: u16,
    /// The error type answered, when it was an error.
    pub error_type: Option<String>,
}

/// One delivery event, and what became of publishing it.
#[derive(Debug, Clone)]
pub struct NotificationRecord {
    /// The SMS `MessageId` the event is about.
    pub message_id: String,
    /// `eventType`.
    pub event_type: String,
    /// `isFinal`.
    pub is_final: bool,
    /// POST attempts made.
    pub attempts: u32,
    /// The last HTTP status the endpoint answered; `None` when every
    /// attempt failed before a response.
    pub last_status: Option<u16>,
    /// Whether an attempt got a 2xx.
    pub acknowledged: bool,
}

/// Everything the fake saw and did. Read this, not the system under test's
/// database, to know what AWS would have recorded.
#[derive(Debug, Default)]
pub struct Ledger {
    submits: Mutex<Vec<SubmitRecord>>,
    notifications: Mutex<Vec<NotificationRecord>>,
    in_flight: AtomicUsize,
}

impl Ledger {
    /// Every submit, accepted or not, in arrival order.
    #[must_use]
    pub fn submits(&self) -> Vec<SubmitRecord> {
        lock(&self.submits).clone()
    }

    /// Submits whose message AWS would now be trying to deliver.
    #[must_use]
    pub fn accepted(&self) -> Vec<SubmitRecord> {
        self.submits()
            .into_iter()
            .filter(|record| record.message_id.is_some())
            .collect()
    }

    /// Every finished notification, in completion order.
    #[must_use]
    pub fn notifications(&self) -> Vec<NotificationRecord> {
        lock(&self.notifications).clone()
    }

    /// Notifications scheduled or being retried, not yet finished.
    #[must_use]
    pub fn notifications_in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub(crate) fn record_submit(&self, record: SubmitRecord) {
        lock(&self.submits).push(record);
    }

    pub(crate) fn notification_scheduled(&self) {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn record_notification(&self, record: NotificationRecord) {
        lock(&self.notifications).push(record);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Every mutation is one push, so a poisoned lock is still consistent.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
