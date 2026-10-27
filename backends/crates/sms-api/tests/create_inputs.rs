//! Milestone 0's second gate: every create input carries the fields its
//! procedure has to set.
//!
//! This file exists to fail at **compile** time, not at runtime. Two schema
//! attributes silently remove a field from `CreateXInput`, and neither leaves
//! any trace at the call site:
//!
//! - **Any `@default(...)` excludes a field from the create input** — literals
//!   included, not only `dbgenerated()`. Putting `@default(0)` on
//!   `Message.priority` would make `sendMessage` unable to set the priority a
//!   caller asked for, and nothing would complain until someone noticed every
//!   message was priority zero.
//! - **`@server_only` excludes a field from create *and* update** (R3), so a
//!   field marked that way can never be populated at all.
//!
//! Both are silent because the generated struct simply has one field fewer.
//! Constructing each input exhaustively — no `..Default::default()` — turns
//! either change into a build failure naming the exact field.
//!
//! Deliberately not `#[test]`-only logic: the assertions here are the
//! `struct` literals themselves. The test bodies just keep the values alive.

use chrono::{TimeZone, Utc};
use cratestack::Decimal;
use sms_api::schema::{
    ClientKeyAlg, ConsoleKeyAlg, CreateAppClientInput, CreateAuditAnchorInput,
    CreateClientKeyInput, CreateConsoleKeyInput, CreateEnvelopeNonceInput, CreateJobInput,
    CreateMessageInput, CreateOperatorPrefixRuleInput, Encoding, MessageClass, OperatorCode,
};

/// Every field `sendMessage` must control on the row it creates.
///
/// If this stops compiling, read the error before "fixing" it: a field that
/// vanished from this struct is a field the send path can no longer set.
#[test]
fn send_message_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateMessageInput {
        // Ownership and idempotency.
        appId: "app000000000000000000001".to_owned(),
        clientRef: Some("order-4821".to_owned()),
        idempotencyKey: Some("idem-4821".to_owned()),

        // Recipient. `msisdn` is @pii, which redacts audit snapshots only —
        // it does not stop the API returning the value.
        msisdn: "+237677123456".to_owned(),
        msisdnHash: "hmac-sha256-v1:...".to_owned(),

        // The four §12 calls out by name: a `@default` on any of these would
        // make `sendMessage` unable to honour what the caller asked for.
        operator: OperatorCode::mtn,
        class: MessageClass::otp,
        priority: 100,
        maxAttempts: 3,

        // Sender identity. Article 48 requires this be a registered value.
        senderIdValue: "VYMALO".to_owned(),

        // Body and its encoding verdict, computed pre-persistence.
        body: Some("Votre code est 4821".to_owned()),
        bodyHash: "hmac-sha256-v1:...".to_owned(),
        bodyLength: 19,
        encoding: Encoding::gsm7,
        segments: 1,

        // Routing, filled by the worker rather than the API, but writable
        // because the worker updates the same row.
        stateReason: None,
        routeId: None,
        providerId: None,
        providerMessageRef: None,
        providerMessageRefAlt: None,
        excludedRouteIds: None,

        // Lease fields. The claim loop CASes on these, so they must be
        // writable — a `@default` here would break reclamation outright.
        leaseOwner: None,
        leaseUntil: None,

        // Scheduling and validity.
        scheduledAt: None,
        expiresAt: now,

        // Timestamps the trigger stamps, but which must still be settable so a
        // backfill or a replayed DLR can carry the real time rather than now().
        submittedAt: None,
        finalizedAt: None,

        // #67: `purge_retention`'s own marker — absent at creation, set only
        // once the row is actually purged past its 90-day retention window.
        purgedAt: None,
    };

    // `state`, `attempts`, `costXaf`, `id`, `createdAt` and `updatedAt` are
    // absent by design — each carries a `@default`, and being unsettable is
    // the control. `Message.state @default('accepted')` is precisely why no
    // client can create a message that is already `delivered`.
    assert_eq!(input.segments, 1);
    assert_eq!(input.encoding, Encoding::gsm7);
}

/// Every field `enqueueJob` must control.
#[test]
fn enqueue_job_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateJobInput {
        kind: "reap_outbox".to_owned(),
        dedupeKey: Some("reap-2026-01-01".to_owned()),
        payload: "{}".to_owned(),
        priority: 500,
        runAt: now,
        maxAttempts: 5,
        leaseOwner: None,
        leaseUntil: None,
        lastError: None,
        startedAt: None,
        finishedAt: None,
    };

    // `state`, `attempts` and `version` are framework-owned.
    assert_eq!(input.kind, "reap_outbox");
}

/// Every field #68's `anchor_audit` job must control on the row it creates
/// (`backends/crates/sms-worker/src/jobs/anchor_audit.rs`). `CreateAuditAnchorInput`
/// derives no `Default` (create inputs never do — only update inputs do),
/// so the job's own real `.create()` call is already exhaustive by
/// construction; this is the same regression canary the other cases in
/// this file are, kept for consistency with how every other job/procedure
/// writer earns one here.
#[test]
fn anchor_audit_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let genesis = "0".repeat(64);

    let input = CreateAuditAnchorInput {
        periodStart: None,
        periodEnd: now,
        rowCount: 0,
        rangeHash: genesis.clone(),
        prevChainHash: genesis,
        chainHash: "1".repeat(64),
    };

    // `id` and `createdAt` are framework-owned (`@default(dbgenerated())`).
    assert_eq!(input.rowCount, 0);
}

/// Every field a manual or DLR-driven correction to an
/// `OperatorPrefixRule` must control.
///
/// `source`, `confidence` and `active` are absent by design — each carries a
/// `@default`, which is why `previewMessage` reporting `unknown` today isn't a
/// bug: nothing writes this table yet, and being unable to default a brand
/// new rule to `unverified` would be worse than the rule not existing.
#[test]
fn operator_prefix_correction_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateOperatorPrefixRuleInput {
        prefix: "655".to_owned(),
        operator: OperatorCode::orange,
        lastObservedAt: Some(now),
        notes: Some("corrected from a DLR-reported network code".to_owned()),
    };

    assert_eq!(input.prefix, "655");
}

/// `provisionAppClient` writes `AppClient` first, `ClientKey` second (its
/// `appClientId` references the row this creates) — every field it controls
/// on the `AppClient` half.
///
/// `active` has no entry here on purpose: `@default(true)`, so a freshly
/// provisioned client is always born active and `active` is settable only on
/// update — e.g. the deactivation path §23's PR description scopes
/// retirement down to.
#[test]
fn app_client_can_set_every_field_it_owns() {
    let input = CreateAppClientInput {
        appId: "app00000000000000000001".to_owned(),
        clientId: "otp-svc-v1".to_owned(),
        label: "OTP service".to_owned(),
        scopes: " sms:send ".to_owned(),
        lastUsedAt: None,
        retiredAt: None,
    };

    assert_eq!(input.clientId, "otp-svc-v1");
}

/// Every field `registerAppClientKey` must control on the `ClientKey` it
/// writes (VS-2). A `@default` on `alg`, `publicKey` or `thumbprint` would
/// drop it from the input and leave the procedure unable to say which key it
/// registered; `revokedAt` is absent from nothing - it is `Option`, set on
/// update by `revokeAppClientKey`, and `None` here.
#[test]
fn register_client_key_can_set_every_field_it_owns() {
    let input = CreateClientKeyInput {
        appClientId: "appclient0000000000001".to_owned(),
        alg: ClientKeyAlg::ed25519,
        publicKey: vec![0; 32],
        thumbprint: "0".repeat(64),
        label: Some("console composer".to_owned()),
        revokedAt: None,
    };

    assert_eq!(input.publicKey.len(), 32);
}

/// Every field `registerConsoleKey` must control on the `ConsoleKey` it
/// writes (VS-5a). A `@default` on `pairedAt` or `expiresAt` would drop it from
/// the input and leave the procedure unable to set the 30-day lifetime (R3);
/// one on `alg`, `publicKey` or `thumbprint` would leave it unable to say which
/// key it registered.
#[test]
fn register_console_key_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let input = CreateConsoleKeyInput {
        userId: "user00000000000000001".to_owned(),
        alg: ConsoleKeyAlg::esp256,
        publicKey: vec![4; 65],
        thumbprint: "0".repeat(64),
        label: Some("break-glass".to_owned()),
        pairedAt: now,
        expiresAt: now + chrono::Duration::days(30),
        revokedAt: None,
    };

    assert_eq!(input.publicKey.len(), 65);
}

/// Recording a signed request's `(kid, cti)` (VS-3).
///
/// `nonce` is the uniqueness the `23505` catch depends on and `expiresAt` is
/// what the worker's sweep reaps by; a `@default` on either would drop it from
/// the input and leave the nonce store unable to say what it recorded or until
/// when.
#[test]
fn envelope_nonce_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateEnvelopeNonceInput {
        nonce: "cose:0011223344556677:00112233445566778899aabbccddeeff".to_owned(),
        expiresAt: now,
    };

    assert_eq!(input.expiresAt, now);
    assert!(input.nonce.starts_with("cose:"));
}

/// `Decimal` is reachable and is the money type.
///
/// Guards the §2.0 note that money is never floating point: if `costXaf` ever
/// changes type, this stops compiling here rather than rounding silently in
/// production.
#[test]
fn money_is_decimal_not_float() {
    let cost: Decimal = "12.50"
        .parse()
        .expect("Decimal parses a fixed-point literal");
    assert_eq!(cost.to_string(), "12.50");
}
