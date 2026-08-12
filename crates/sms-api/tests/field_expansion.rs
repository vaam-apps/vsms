//! #24's second half of *"a test asserting the full generated policy set
//! and expanded field list, so a typo'd `@@allow` or `@@use` fails the
//! build rather than silently no-opping."* This file is the `@@use`/`@use`
//! half; `tests/policy_golden_list_live_postgres.rs` is the `@@allow` half.
//!
//! Same technique as `tests/create_inputs.rs`, mirrored onto the *read*
//! side: `create_inputs.rs` exhaustively constructs `CreateXInput` structs
//! (no `..Default::default()`) so a field silently dropped by `@default`/
//! `@server_only` fails to compile. That technique cannot catch *this*
//! trap, though — §2.0 is explicit that `@@use` (double `@`) instead of
//! `@use` (single `@`) "does not expand the mixin. No error", and the one
//! mixin in this schema (`Timestamps`: `createdAt`/`updatedAt`) contributes
//! only `@default(dbgenerated())` fields, which `@default`'s own rule
//! *already* excludes from `CreateXInput` — a silently-unexpanded mixin and
//! a correctly-expanded one produce an *identical* `CreateXInput`, so
//! `create_inputs.rs` would never notice the difference.
//!
//! What *would* notice: the row struct a query returns. If `@use(Timestamps)`
//! fails to expand, `schema::App` (etc.) simply has no `createdAt`/
//! `updatedAt` fields at all — a fact `Default`-style destructuring makes a
//! compile error, the same "fails at compile time, not at runtime" property
//! `create_inputs.rs` already relies on. These functions are never called;
//! they exist to be type-checked. Deliberately not every one of the 19
//! models — four representative ones, chosen the same way
//! `create_inputs.rs` chose six: the models this PR's own history (#24 and
//! the policy gaps documented across `schema.cstack`) actually touches or
//! has previously gotten wrong. A fifth or twentieth model needs only
//! another function in this same shape, not a new mechanism.

#[allow(dead_code, unreachable_code, clippy::diverging_sub_expression)]
fn assert_app_row_carries_every_expected_field(app: sms_api::schema::App) {
    let sms_api::schema::App {
        id,
        name,
        slug,
        description,
        defaultSenderIdId,
        monthlyQuota,
        ipAllowlist,
        transliterateToGsm7,
        active,
        deletedAt,
        // #59: App is one of ten operator-editable models that gained
        // `@version` so PATCH routes can require If-Match.
        version,
        // If `@use(Timestamps)` silently didn't expand, these two names
        // stop existing on this struct and this function stops compiling.
        createdAt,
        updatedAt,
    } = app;
    let _ = (
        id,
        name,
        slug,
        description,
        defaultSenderIdId,
        monthlyQuota,
        ipAllowlist,
        transliterateToGsm7,
        active,
        deletedAt,
        version,
        createdAt,
        updatedAt,
    );
}

#[allow(dead_code, unreachable_code, clippy::diverging_sub_expression)]
fn assert_app_client_row_carries_every_expected_field(app_client: sms_api::schema::AppClient) {
    let sms_api::schema::AppClient {
        id,
        appId,
        clientId,
        label,
        scopes,
        active,
        lastUsedAt,
        retiredAt,
        version,
        createdAt,
        updatedAt,
    } = app_client;
    let _ = (
        id, appId, clientId, label, scopes, active, lastUsedAt, retiredAt, version, createdAt,
        updatedAt,
    );
}

#[allow(dead_code, unreachable_code, clippy::diverging_sub_expression)]
fn assert_provider_row_carries_every_expected_field(provider: sms_api::schema::Provider) {
    let sms_api::schema::Provider {
        id,
        key,
        displayName,
        kind,
        state,
        config,
        credentialRef,
        maxTps,
        maxDailySubmissions,
        supportsDlr,
        supportsAlphaSender,
        supportsUcs2,
        supportsConcat,
        costPerSegmentXaf,
        healthCheckedAt,
        healthy,
        version,
        createdAt,
        updatedAt,
    } = provider;
    let _ = (
        id,
        key,
        displayName,
        kind,
        state,
        config,
        credentialRef,
        maxTps,
        maxDailySubmissions,
        supportsDlr,
        supportsAlphaSender,
        supportsUcs2,
        supportsConcat,
        costPerSegmentXaf,
        healthCheckedAt,
        healthy,
        version,
        createdAt,
        updatedAt,
    );
}

#[allow(dead_code, unreachable_code, clippy::diverging_sub_expression)]
fn assert_message_row_carries_every_expected_field(message: sms_api::schema::Message) {
    let sms_api::schema::Message {
        id,
        appId,
        clientRef,
        idempotencyKey,
        msisdn,
        msisdnHash,
        operator,
        senderIdValue,
        class,
        priority,
        body,
        bodyHash,
        bodyLength,
        encoding,
        segments,
        state,
        stateReason,
        routeId,
        providerId,
        providerMessageRef,
        providerMessageRefAlt,
        attempts,
        maxAttempts,
        leaseOwner,
        leaseUntil,
        scheduledAt,
        expiresAt,
        submittedAt,
        finalizedAt,
        purgedAt,
        costXaf,
        version,
        createdAt,
        updatedAt,
    } = message;
    let _ = (
        id,
        appId,
        clientRef,
        idempotencyKey,
        msisdn,
        msisdnHash,
        operator,
        senderIdValue,
        class,
        priority,
        body,
        bodyHash,
        bodyLength,
        encoding,
        segments,
        state,
        stateReason,
        routeId,
        providerId,
        providerMessageRef,
        providerMessageRefAlt,
        attempts,
        maxAttempts,
        leaseOwner,
        leaseUntil,
        scheduledAt,
        expiresAt,
        submittedAt,
        finalizedAt,
        purgedAt,
        costXaf,
        version,
        createdAt,
        updatedAt,
    );
}

/// Deliberately not `#[test]`-only logic (same note `create_inputs.rs`
/// opens with): the assertions above are the function signatures and
/// destructuring patterns themselves, checked by `cargo check`/`cargo
/// build`, not by running anything. This test just keeps the crate honest
/// that the functions above exist and are reachable from a test binary.
#[test]
fn field_expansion_assertions_compile() {}
