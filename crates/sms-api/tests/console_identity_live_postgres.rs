//! #52/#58: `provisionUser`, `recordOptOut`, `searchOptOutByMsisdn` — the
//! `ProcedureRegistry` trait methods, not the inherent `Procedures` methods
//! directly, the same discipline `send_message_live_postgres.rs` documents
//! in its own module doc.
//!
//! ```bash
//! cargo test -p sms-api --test console_identity_live_postgres -- --ignored
//! ```

use chrono::Utc;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, Value};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self,
    procedures::{provision_user, record_opt_out, search_opt_out_by_msisdn, ProcedureRegistry},
    Cratestack,
};
use sms_api::{HashPepper, Procedures};

/// Same reasoning as every other live suite's own copy of this mutex — see
/// `claim_live_postgres.rs`'s own `TEST_MUTEX` doc (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn test_pepper() -> HashPepper {
    HashPepper::new("console-identity-live-postgres-test-pepper-well-over-minimum")
        .expect("test pepper meets HashPepper::new's minimum length")
}

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

fn owner() -> CoolContext {
    Principal {
        sub: "console-identity-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// A test-constructed context, unlike a real issued token, carries no
/// `perms` claim unless one is added by hand — see
/// `rotate_webhook_secret_live_postgres.rs`'s own
/// `owner_with_webhook_manage` for the precedent this mirrors.
fn owner_with_user_manage() -> CoolContext {
    let mut ctx = owner();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("user:manage".to_owned())]),
    );
    ctx
}

fn operator_with_optout_manage() -> CoolContext {
    let mut ctx = Principal {
        sub: "console-identity-test-operator".to_owned(),
        kind: PrincipalKind::User,
        role: "operator".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("optout:manage".to_owned())]),
    );
    ctx
}

/// The bare role, no `perms` claim at all — Layer 1 alone would admit this
/// caller (`operator` is in every relevant procedure's own `@allow`), but
/// Layer 2's `require_permission(ctx, "optout:manage")` must still deny it.
fn operator_without_permission() -> CoolContext {
    Principal {
        sub: "console-identity-test-operator-no-perms".to_owned(),
        kind: PrincipalKind::User,
        role: "operator".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// `support` — #58's own finding: `OptOut.create`'s own `@@allow` never
/// admitted `hasRole('support')`, even though §5.2 grants that role
/// `optout:manage` specifically because opt-outs are its stated job. This
/// context is what proves `createOptOutEntry`'s `sys()`-context write
/// closes that gap without touching the model's own policy.
fn support_with_optout_manage() -> CoolContext {
    let mut ctx = Principal {
        sub: "console-identity-test-support".to_owned(),
        kind: PrincipalKind::User,
        role: "support".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "perms".to_owned(),
        Value::List(vec![Value::String("optout:manage".to_owned())]),
    );
    ctx
}

async fn seed_role(db: &Cratestack, key: &str, permissions: &[&str]) {
    let packed = sms_core::pack(permissions).expect("test permission literals contain no space");
    match db
        .role()
        .create(schema::CreateRoleInput {
            key: key.to_owned(),
            label: format!("console identity test role {key}"),
            description: None,
            permissions: packed,
        })
        .run(&owner())
        .await
    {
        Ok(_) => {}
        Err(e) if e.db_sqlstate() == Some("23505") => {
            // Already seeded by an earlier run against this shared
            // database — fine, this suite doesn't depend on which attempt
            // won.
        }
        Err(e) => panic!("seeding role {key}: {e:?}"),
    }
}

// ---------------------------------------------------------------------
// provisionUser (#52/#58)
// ---------------------------------------------------------------------

/// The headline case: `provisionUser` returns a plaintext password exactly
/// once, and that password genuinely authenticates against the account it
/// just created — proven by calling `sms_auth::login::authenticate_user`
/// directly, the real login check, not a hand-rolled stand-in for it.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_provisioned_user_can_actually_log_in_with_the_returned_password() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    seed_role(&db, "console_identity_owner_role", &["user:manage"]).await;

    let suffix = unique_suffix();
    let email = format!("provisioned-{suffix}@example.test");

    let result = Procedures::new(test_pepper())
        .provision_user(
            &db,
            &owner_with_user_manage(),
            provision_user::Args {
                args: schema::ProvisionUserInput {
                    email: email.clone(),
                    displayName: "Provisioned Test User".to_owned(),
                    roleKey: "console_identity_owner_role".to_owned(),
                },
            },
        )
        .await
        .expect("provisioning a user");

    assert_eq!(result.email, email);
    assert!(!result.password.is_empty());
    assert_eq!(
        result.password.len(),
        24,
        "sms_core::password::generate_password's own documented length"
    );

    let sys = Principal {
        sub: "console-identity-test-login-sys".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context();

    let authenticated = sms_auth::login::authenticate_user(&db, &sys, &email, &result.password)
        .await
        .expect(
            "the returned one-time password must authenticate against the account it just created",
        );
    assert_eq!(authenticated.subject, result.userId);
    assert_eq!(authenticated.role_key, "console_identity_owner_role");

    // The wrong password, against the same freshly provisioned account,
    // must still fail — proves this isn't a login check that accepts
    // anything for a freshly created row.
    let wrong = sms_auth::login::authenticate_user(&db, &sys, &email, "definitely-not-it").await;
    assert!(wrong.is_err(), "a wrong password must not authenticate");
}

/// A second `provisionUser` call for the same email must be a clear
/// conflict (`User.email` is `@unique`), not a raw 500 — proves
/// `map_database_error`'s `23505` mapping reaches this new write path too.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn provisioning_the_same_email_twice_is_a_conflict() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    seed_role(&db, "console_identity_dup_role", &["user:manage"]).await;
    let procedures = Procedures::new(test_pepper());

    let email = format!("dup-{}@example.test", unique_suffix());
    let args = || provision_user::Args {
        args: schema::ProvisionUserInput {
            email: email.clone(),
            displayName: "Dup Test".to_owned(),
            roleKey: "console_identity_dup_role".to_owned(),
        },
    };

    procedures
        .provision_user(&db, &owner_with_user_manage(), args())
        .await
        .expect("first provision succeeds");

    let second = procedures
        .provision_user(&db, &owner_with_user_manage(), args())
        .await;
    assert!(
        matches!(second, Err(cratestack::CoolError::Conflict(_))),
        "expected a named Conflict on a duplicate email, got {second:?}"
    );
}

// ---------------------------------------------------------------------
// recordOptOut / searchOptOutByMsisdn (#58)
// ---------------------------------------------------------------------

fn test_msisdn(suffix: &str) -> String {
    // A syntactically valid Cameroon MTN mobile number (677 prefix, 9
    // national digits total per §3.4) — `suffix` (3 digits) fills the last
    // three, keeping numbers distinct per test.
    format!("+237677000{suffix}")
}

/// Recording an opt-out and then searching for the exact same number finds
/// it — the round trip `schema.cstack`'s own doc on these two procedures
/// promises.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn recording_an_opt_out_makes_it_findable_by_the_same_number() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());
    let msisdn = test_msisdn("001");

    let recorded = procedures
        .record_opt_out(
            &db,
            &operator_with_optout_manage(),
            record_opt_out::Args {
                args: schema::RecordOptOutInput {
                    msisdn: msisdn.clone(),
                    source: schema::OptOutSource::admin,
                    scope: "all".to_owned(),
                    reason: Some("test seed".to_owned()),
                },
            },
        )
        .await
        .expect("recording an opt-out");
    assert_eq!(recorded.msisdn, "+237677000001");

    let found = procedures
        .search_opt_out_by_msisdn(
            &db,
            &operator_with_optout_manage(),
            search_opt_out_by_msisdn::Args {
                args: schema::OptOutSearchInput {
                    msisdn: msisdn.clone(),
                },
            },
        )
        .await
        .expect("searching for the just-recorded number");

    let summary = found.optOut.expect("the recorded opt-out must be found");
    assert_eq!(summary.id, recorded.id);
    assert_eq!(summary.msisdnHash, recorded.msisdnHash);
}

/// **The house standard: a search that matches nothing must return
/// nothing, never fall back to an unfiltered list.** Seeds several
/// unrelated opt-outs, then searches for a number that was never recorded
/// — the result must be structurally `None`, not "the first row the table
/// happens to have."
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn searching_an_unrecorded_number_finds_nothing_even_with_other_rows_present() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    // Seed three unrelated opt-outs first, so an unfiltered fallback would
    // have something to wrongly return.
    for suffix in ["101", "102", "103"] {
        procedures
            .record_opt_out(
                &db,
                &operator_with_optout_manage(),
                record_opt_out::Args {
                    args: schema::RecordOptOutInput {
                        msisdn: test_msisdn(suffix),
                        source: schema::OptOutSource::admin,
                        scope: "all".to_owned(),
                        reason: None,
                    },
                },
            )
            .await
            .expect("seeding an unrelated opt-out");
    }

    let never_recorded = test_msisdn("999");
    let result = procedures
        .search_opt_out_by_msisdn(
            &db,
            &operator_with_optout_manage(),
            search_opt_out_by_msisdn::Args {
                args: schema::OptOutSearchInput {
                    msisdn: never_recorded,
                },
            },
        )
        .await
        .expect("searching for a number that was never recorded");

    assert!(
        result.optOut.is_none(),
        "a search that matches nothing must return None, not one of the unrelated seeded rows: \
         got {:?}",
        result.optOut
    );
}

/// #58's own finding: `OptOut.create`'s own `@@allow` never admitted
/// `hasRole('support')`, even though §5.2 grants that role `optout:manage`
/// specifically for this. `createOptOutEntry` runs its write under `sys`
/// regardless of the caller's own role, which is what makes this succeed
/// — a direct `POST /opt_outs` as `support` would still be refused by the
/// model's own Layer 1 policy, untouched by this PR.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn a_support_role_caller_can_record_an_opt_out_the_bare_model_policy_would_refuse() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    let recorded = procedures
        .record_opt_out(
            &db,
            &support_with_optout_manage(),
            record_opt_out::Args {
                args: schema::RecordOptOutInput {
                    msisdn: test_msisdn("201"),
                    source: schema::OptOutSource::inbound_stop,
                    scope: "all".to_owned(),
                    reason: None,
                },
            },
        )
        .await
        .expect("a support-role caller with optout:manage must be able to record an opt-out");
    assert_eq!(recorded.source, schema::OptOutSource::inbound_stop);
}

/// Layer 2, proven the same way `rotate_webhook_secret_live_postgres.rs`
/// proves its own `webhook:manage` gate: a caller whose *role* Layer 1
/// would admit, but who carries no `perms` claim at all, must still be
/// denied by `require_permission`.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn recording_or_searching_denies_a_caller_with_no_optout_manage_permission() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    let record_result = procedures
        .record_opt_out(
            &db,
            &operator_without_permission(),
            record_opt_out::Args {
                args: schema::RecordOptOutInput {
                    msisdn: test_msisdn("301"),
                    source: schema::OptOutSource::admin,
                    scope: "all".to_owned(),
                    reason: None,
                },
            },
        )
        .await;
    assert!(
        matches!(record_result, Err(cratestack::CoolError::Forbidden(_))),
        "expected Forbidden, got {record_result:?}"
    );

    let search_result = procedures
        .search_opt_out_by_msisdn(
            &db,
            &operator_without_permission(),
            search_opt_out_by_msisdn::Args {
                args: schema::OptOutSearchInput {
                    msisdn: test_msisdn("301"),
                },
            },
        )
        .await;
    assert!(
        matches!(search_result, Err(cratestack::CoolError::Forbidden(_))),
        "expected Forbidden, got {search_result:?}"
    );
}

/// `recordOptOut` stamps `optedOutAt` at call time, not left null or
/// defaulted to the epoch — a real, current timestamp a search result can
/// be trusted to sort/filter by.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn recording_an_opt_out_stamps_a_current_opted_out_at() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    let before = Utc::now();
    let recorded = procedures
        .record_opt_out(
            &db,
            &operator_with_optout_manage(),
            record_opt_out::Args {
                args: schema::RecordOptOutInput {
                    msisdn: test_msisdn("401"),
                    source: schema::OptOutSource::admin,
                    scope: "all".to_owned(),
                    reason: None,
                },
            },
        )
        .await
        .expect("recording an opt-out");
    let after = Utc::now();

    assert!(recorded.optedOutAt >= before && recorded.optedOutAt <= after);
}
