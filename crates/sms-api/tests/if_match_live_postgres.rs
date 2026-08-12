//! #59: "thread `ETag` / If-Match through every edit" — proved at the layer
//! that is actually real and reachable in this deployment.
//!
//! # Why this test calls the delegate directly, not `PATCH /providers/{id}`
//! over real HTTP
//!
//! `crates/sms-auth/tests/rbac_layer2_live_postgres.rs` and
//! `app/sms-gateway/tests/m1_acceptance_gate_live_postgres.rs` both already
//! document, independently, that no real bearer token this deployment can
//! issue ever reaches a Layer-1 *allow* on `PATCH /providers/{id}` —
//! `Provider.update`'s own `@@allow` needs `hasRole('owner')`,
//! `hasRole('admin')`, or `hasRole('operator')`, and `GatewayAuth` is the
//! only `AuthProvider` in this codebase (`grep -rn "impl AuthProvider"`
//! confirms it — one hit). Reading `GatewayAuth::authenticate`
//! (`crates/sms-api/src/auth.rs`) resolves the "why" precisely, and it is
//! stronger than either of those two files states it: `role` is not read
//! from any JWT claim at all — `role: "app".to_owned()` is a literal
//! constant on every real token this deployment ever mints, `perms`/`scope`
//! land in `ctx.extensions` for Layer 2, never in the four `auth.fields`
//! Layer 1's `hasRole(...)` reads. So a *hand-signed* stand-in token (the
//! technique `m1_acceptance_gate_live_postgres.rs`'s own
//! `sign_developer_stand_in_token` uses to reach a Layer-2 *denial* on this
//! same route) cannot help here either: forging claims changes what a real
//! token would have carried, not what `GatewayAuth` derives from it, and
//! `role` isn't derived from the token at all. Every one of #59's ten newly
//! `@version`'d models has the identical shape (`SenderId`'s `update` policy
//! is `hasRole('owner') || hasRole('admin') || hasRole('operator')`,
//! `WebhookEndpoint`'s adds `hasRole('developer')` — never `"app"`), so this
//! is not a Provider-specific gap: **no write route this ticket touches is
//! reachable over real HTTP by any token this deployment can currently
//! issue**, full stop, until a human-login `AuthProvider` exists (tracked
//! separately, same scope cut `#24`/`#25` already recorded).
//!
//! What *is* real and reachable: the delegate call the generated `PATCH`
//! handler makes internally. `cratestack-macros-0.7.10/src/axum/model/prep/
//! etag.rs` (read directly, not assumed) shows the generated handler does
//! exactly three things around the handler body this test exercises
//! directly — parse `If-Match` into `Option<i64>` (missing header ==>
//! `CoolError::PreconditionFailed("If-Match header required")`, the same
//! error this test asserts below), call `.if_match(version)` on the same
//! `UpdateRecordSet` builder `db.provider().update(id).set(...)` returns
//! here, and stamp the response `ETag` from the returned row's own
//! `version` field on success. This test proves the one part of that chain
//! that is genuinely data — the CAS semantics against a live database — the
//! same way `policy_golden_list_live_postgres.rs` already proves this
//! model's `@@allow` policies live, for the identical reason: the thing
//! that needs a real Postgres to prove isn't reachable through the one
//! piece of this deployment (`GatewayAuth`) that structurally cannot get
//! there yet.
//!
//! `CoolError::PreconditionFailed`'s HTTP shape (412, code
//! `"PRECONDITION_FAILED"`) is asserted directly against the framework's
//! own `status_code()`/`code()` methods below — `cratestack-core-0.7.10/
//! src/error.rs`, not this crate's own `errors.rs` (which only maps
//! *database*-level SQLSTATEs; a losing `if_match` is a plain
//! `Err(CoolError::PreconditionFailed(...))` returned before any SQL runs,
//! see `cratestack-sqlx-0.7.10/src/query/write/update.rs`'s own
//! `run_in_tx`) — so this test is checking the framework's real behaviour,
//! not restating this crate's own code back at itself.
//!
//! Ignored by default, same convention as this workspace's other live
//! suites. Run explicitly:
//!
//! ```bash
//! cargo test -p sms-api --test if_match_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};

/// Same reasoning as every other live suite's own copy of this mutex —
/// see `crates/sms-worker/tests/claim_live_postgres.rs`'s doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

/// `owner` is the loosest of `Provider.update`'s three admitted roles —
/// which role is used doesn't matter to this file, only that CAS behaves
/// the same regardless (Layer 1's own admission is `policy_golden_list_
/// live_postgres.rs`'s job, not this file's).
fn owner() -> CoolContext {
    Principal {
        sub: "if-match-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn fresh_provider_input() -> schema::CreateProviderInput {
    let suffix = unique_suffix();
    schema::CreateProviderInput {
        key: format!(
            "ifmatch_{}",
            suffix.to_lowercase().chars().take(20).collect::<String>()
        ),
        displayName: "If-Match Test Provider".to_owned(),
        kind: schema::ProviderKind::aggregator_http,
        config: "{}".to_owned(),
        credentialRef: "vault://test".to_owned(),
        maxTps: 5.0,
        maxDailySubmissions: 1000,
        supportsDlr: true,
        supportsAlphaSender: true,
        supportsUcs2: true,
        supportsConcat: true,
        costPerSegmentXaf: "15".parse().unwrap(),
        healthCheckedAt: None,
    }
}

/// The ticket's own worked example, reproduced for real: two consoles GET
/// the same row (so both capture the same `ETag`/version), one `PATCH`es
/// successfully, and the second's now-stale PATCH must be rejected with a
/// genuine 412 — not silently overwrite the first operator's change.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn two_operators_editing_the_same_provider_row_the_second_stale_write_gets_412() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    // Both operators' consoles load the row — this is the moment a real
    // `GET /providers/{id}` would hand each browser tab an `ETag` header
    // carrying this exact `version`.
    let seeded = db
        .provider()
        .create(fresh_provider_input())
        .run(&owner())
        .await
        .expect("seeding a provider");
    // Both operators' consoles capture this exact value — a single shared
    // binding is deliberate: it's what makes the two writes below a genuine
    // race over the same starting point, not two independent updates.
    let shared_starting_version = seeded.version;

    // Operator A saves first — the equivalent of a real `PATCH
    // /providers/{id}` carrying `If-Match: "0"`.
    let after_a = db
        .provider()
        .update(seeded.id.clone())
        .set(schema::UpdateProviderInput {
            displayName: Some("Renamed by operator A".to_owned()),
            ..Default::default()
        })
        .if_match(shared_starting_version)
        .run(&owner())
        .await
        .expect("operator A's write must succeed — nothing raced it yet");
    assert_eq!(
        after_a.version,
        shared_starting_version + 1,
        "a successful update must advance the version by exactly one"
    );
    assert_eq!(after_a.displayName, "Renamed by operator A");

    // Operator B's browser tab has been open the whole time and still
    // carries the *original* ETag — the equivalent of a real `PATCH
    // /providers/{id}` still sending `If-Match: "0"`, now stale.
    let b_result = db
        .provider()
        .update(seeded.id.clone())
        .set(schema::UpdateProviderInput {
            displayName: Some("Renamed by operator B".to_owned()),
            ..Default::default()
        })
        .if_match(shared_starting_version)
        .run(&owner())
        .await;

    let error = b_result.expect_err(
        "operator B's stale write must be rejected, not silently overwrite operator A's change",
    );
    assert!(
        matches!(error, CoolError::PreconditionFailed(_)),
        "expected PreconditionFailed, got {error:?}"
    );
    // The framework's own HTTP shape (cratestack-core's error.rs), asserted
    // directly rather than assumed — this is the fact a browser's `fetch`
    // actually observes on the wire.
    assert_eq!(
        error.status_code().as_u16(),
        412,
        "PreconditionFailed must map to HTTP 412, not some other status"
    );
    assert_eq!(error.code(), "PRECONDITION_FAILED");

    // And the row itself: still operator A's write, never touched by B's
    // rejected one. This is the actual claim "412, not a silent
    // last-write-wins" is making — worth reading back and checking, not
    // just trusting the error variant.
    let current = db
        .provider()
        .find_many()
        .where_expr(cratestack::FilterExpr::from(
            schema::provider::id().eq(seeded.id.clone()),
        ))
        .limit(1)
        .run(&owner())
        .await
        .expect("reading the row back")
        .into_iter()
        .next()
        .expect("the row still exists");
    assert_eq!(
        current.displayName, "Renamed by operator A",
        "operator B's rejected write must not have reached the row"
    );
    assert_eq!(current.version, shared_starting_version + 1);
}

/// The positive case in isolation, without a race — a correct `if_match`
/// always succeeds and the version always advances by exactly one,
/// regardless of how many fields changed in the same write.
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn a_correct_if_match_succeeds_and_the_version_advances_by_one() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let seeded = db
        .provider()
        .create(fresh_provider_input())
        .run(&owner())
        .await
        .expect("seeding a provider");

    let updated = db
        .provider()
        .update(seeded.id.clone())
        .set(schema::UpdateProviderInput {
            maxTps: Some(9.0),
            maxDailySubmissions: Some(2000),
            ..Default::default()
        })
        .if_match(seeded.version)
        .run(&owner())
        .await
        .expect("a correct if_match must succeed");

    assert_eq!(updated.version, seeded.version + 1);
    assert!(
        (updated.maxTps - 9.0).abs() < f64::EPSILON,
        "expected maxTps 9.0, got {}",
        updated.maxTps
    );
    assert_eq!(updated.maxDailySubmissions, 2000);
}

/// The generated `PATCH` handler's own behaviour on a missing `If-Match`
/// header (`cratestack-macros`'s `etag.rs`: `Ok(None) => ...
/// PreconditionFailed("If-Match header required")`) is a thin wrapper over
/// this exact same builder-level check — proved here at the layer that
/// doesn't need a bound HTTP server to exercise: omitting `.if_match(...)`
/// entirely on a `@version`'d model's update is rejected the same way a
/// stale one is, not silently treated as "no precondition, just write it".
#[tokio::test]
#[ignore = "needs a live, fully migrated Postgres — see module docs"]
async fn omitting_if_match_entirely_is_rejected_not_silently_allowed() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let seeded = db
        .provider()
        .create(fresh_provider_input())
        .run(&owner())
        .await
        .expect("seeding a provider");

    let result = db
        .provider()
        .update(seeded.id.clone())
        .set(schema::UpdateProviderInput {
            maxTps: Some(3.0),
            ..Default::default()
        })
        // Deliberately no `.if_match(...)` call.
        .run(&owner())
        .await;

    let error = result.expect_err("a versioned model's update with no If-Match must be rejected");
    assert!(
        matches!(error, CoolError::PreconditionFailed(_)),
        "expected PreconditionFailed, got {error:?}"
    );
    assert_eq!(error.status_code().as_u16(), 412);
}
