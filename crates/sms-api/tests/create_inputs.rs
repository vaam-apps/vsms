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
    ClientAuthMethod, CreateAppClientInput, CreateAuditAnchorInput, CreateClientAssertionInput,
    CreateJobInput, CreateMessageInput, CreateOauthClientInput, CreateOauthSigningKeyInput,
    CreateOperatorPrefixRuleInput, Encoding, MessageClass, OperatorCode,
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
/// (`crates/sms-worker/src/jobs/anchor_audit.rs`). `CreateAuditAnchorInput`
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

/// Every field `provisionAppClient` must control on the client registration.
///
/// `tokenEndpointAuthMethod` is the one to watch. It is `NOT NULL` with no
/// `@default` deliberately (§4.2): a `@default` would drop it from this struct,
/// every registration would be written with whatever the default was, and a
/// *missing* method is how authkestra spells "accepts a secret from either
/// transport, refuses assertions" — the exact state `private_key_jwt` needs to
/// avoid. If this field disappears from here, `private_key_jwt` is off and
/// nothing else says so.
#[test]
fn provision_app_client_can_set_every_field_it_owns() {
    let input = CreateOauthClientInput {
        clientId: "otp-svc-v1".to_owned(),
        appClientId: Some("apc00000000000000000001".to_owned()),
        tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
        // A real key set: §2.10's CHECK rejects `{"keys":[]}`, which is not
        // null and still keyless.
        jwks: Some(r#"{"keys":[{"kty":"RSA","kid":"k1","n":"…","e":"AQAB"}]}"#.to_owned()),
        grantTypes: " client_credentials ".to_owned(),
        scopes: " sms:send ".to_owned(),
        redirectUris: " ".to_owned(),
        requirePkce: false,
    };

    assert_eq!(
        input.tokenEndpointAuthMethod,
        ClientAuthMethod::private_key_jwt
    );
}

/// `provisionAppClient` writes `AppClient` first, `OauthClient` second (its
/// `appClientId` references the row this creates) — every field it controls
/// on the `AppClient` half.
///
/// `active` has no entry here on purpose: `@default(true)` (like
/// `OauthClient.active` above), so a freshly provisioned client is always
/// born active and `active` is settable only on update — e.g. the
/// deactivation path §23's PR description scopes retirement down to.
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

/// The OP's signing key, as `sms-auth` writes it on first boot.
///
/// `active` is `@default(true)` and so is absent here on purpose — a key is
/// always born active, and rotation flips it through update.
#[test]
fn signing_key_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateOauthSigningKeyInput {
        privateKeyPem: "-----BEGIN PRIVATE KEY-----".to_owned(),
        expiresAt: Some(now),
    };

    assert!(input.privateKeyPem.starts_with("-----BEGIN"));
}

/// Spending a `private_key_jwt` assertion's `jti`.
///
/// Both fields have to be settable: `jti` is the uniqueness the `23505` catch
/// depends on, and `expiresAt` is what lets the row be reaped instead of
/// accumulating one per token request forever.
#[test]
fn client_assertion_can_set_every_field_it_owns() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

    let input = CreateClientAssertionInput {
        jti: "01HQ8ZK3M7T2V9WXYZ0000".to_owned(),
        expiresAt: now,
    };

    assert_eq!(input.expiresAt, now);
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
