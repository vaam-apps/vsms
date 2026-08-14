//! `auditLog` and `auditChainStatus` (#58) against a real, fully migrated
//! Postgres — the actual `ProcedureRegistry` trait methods, not
//! `Procedures`' own inherent methods called directly, the same discipline
//! `send_message_live_postgres.rs` documents in its own module doc.
//!
//! The house standard this task exists to satisfy — proving the audit view
//! is genuinely read-only, not merely that no write path was built —
//! [`no_role_including_system_can_write_an_audit_anchor`] is that proof.
//! See `backends/crates/sms-api/src/audit_log.rs`'s own module doc for the full
//! story: the first draft of this doc comment claimed a compile-time
//! guard existed (`.update()` wouldn't even compile), which turned out to
//! be wrong the moment it was tried for real — `cratestack-macros`
//! generates the method unconditionally. The real guard is a runtime
//! `Forbidden`, for *every* role including the synthetic `system` context
//! this crate's own procedures use internally, and this test is what pins
//! that down as a permanent assertion rather than a one-off finding.
//!
//! ```bash
//! cargo test -p sms-api --test audit_chain_status_and_audit_log_live_postgres -- --ignored
//! ```

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, CoolError};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self,
    procedures::{audit_chain_status, audit_log, ProcedureRegistry},
    Cratestack, UpdateAuditAnchorInput,
};
use sms_api::{HashPepper, Procedures};

static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn test_pepper() -> HashPepper {
    HashPepper::new("audit-log-live-postgres-test-pepper-well-over-the-minimum-length")
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
        sub: "audit-log-test-owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn owner_with_audit_read() -> CoolContext {
    let mut ctx = owner();
    ctx.extensions.insert(
        "perms".to_owned(),
        cratestack::Value::List(vec![cratestack::Value::String("audit:read".to_owned())]),
    );
    ctx
}

fn sys() -> CoolContext {
    Principal {
        sub: "audit-log-test-sys".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// `App` carries `@@audit`, so creating one always writes a real
/// `cratestack_audit` row `auditLog` can find.
async fn seed_audited_app(db: &Cratestack) -> (String, String) {
    let slug = format!("audit-log-test-{}", unique_suffix());
    let app = db
        .app()
        .create(schema::CreateAppInput {
            name: "audit log test app".to_owned(),
            slug: slug.clone(),
            description: None,
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(&owner())
        .await
        .expect("seeding an audited app");
    (app.id, slug)
}

/// `auditLog` can find a just-written row and describes it correctly —
/// `model` matches, `primaryKey`/`actor` are non-empty JSON, `after`
/// carries the newly created row's own slug.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn audit_log_finds_a_freshly_audited_write_filtered_by_model() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let (app_id, slug) = seed_audited_app(&db).await;

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db` — the "sanctioned way to invoke a procedure from
    // non-HTTP code" per that function's own doc comment.
    let procedures = Procedures::new(test_pepper());
    let ctx = owner_with_audit_read();
    let args = audit_log::Args {
        args: schema::AuditLogQuery {
            model: Some("App".to_owned()),
            operation: Some("create".to_owned()),
            actorId: None,
            since: None,
            until: None,
            limit: Some(200),
            offset: None,
        },
    };
    let page = audit_log::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.audit_log(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("listing the audit log filtered by model");

    let entry = page
        .entries
        .iter()
        .find(|entry| entry.primaryKey.contains(&app_id))
        .unwrap_or_else(|| {
            panic!(
                "expected an App/create audit entry naming id {app_id:?} among {} entries",
                page.entries.len()
            )
        });
    assert_eq!(entry.model, "App");
    assert_eq!(entry.operation, "create");
    assert!(
        entry
            .after
            .as_deref()
            .is_some_and(|after| after.contains(&slug)),
        "the 'after' snapshot should carry the row's own slug: {:?}",
        entry.after
    );
    assert!(
        !entry.actor.is_empty(),
        "actor must be a real JSON blob, not empty"
    );
}

/// A caller without `audit:read` is denied — Layer 2, the same shape every
/// other permission-gated procedure in this file proves.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn audit_log_denies_a_caller_with_no_audit_read_permission() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    // cratestack 0.7.13 (cratestack#512): calling the trait method directly
    // now requires an `Authorized` witness, obtainable only through
    // `invoke_with_db`, which runs the real Layer 1 `@allow` check first —
    // `hasRole('owner')` already admits this caller there (`schema.cstack`'s
    // `auditLog` `@allow`), and `auditLog` carries no `@authorize` model
    // check, so this stays a genuine Layer 2 (`require_permission`) denial.
    let ctx = owner(); // no perms claim at all
    let args = audit_log::Args {
        args: schema::AuditLogQuery {
            model: None,
            operation: None,
            actorId: None,
            since: None,
            until: None,
            limit: None,
            offset: None,
        },
    };
    let result = audit_log::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.audit_log(&db, &ctx, args.clone(), authorized)
    })
    .await;
    assert!(
        matches!(result, Err(CoolError::Forbidden(_))),
        "expected Forbidden, got {result:?}"
    );
}

/// On this binary's own, never-anchored database (nothing in this file
/// ever runs the `anchor_audit` job — only the read-only procedures under
/// test), `auditChainStatus` must report "no anchor yet" honestly, not an
/// error and not a fabricated one.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn audit_chain_status_reports_no_anchor_on_a_never_anchored_database() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;
    let procedures = Procedures::new(test_pepper());

    // cratestack 0.7.13 (cratestack#512): see the identical comment on
    // `audit_log_finds_a_freshly_audited_write_filtered_by_model` above.
    let ctx = owner_with_audit_read();
    let args = audit_chain_status::Args {};
    let status = audit_chain_status::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.audit_chain_status(&db, &ctx, args.clone(), authorized)
    })
    .await
    .expect("audit chain status must succeed even with no anchor yet");

    assert!(status.latestAnchorId.is_none());
    assert!(
        status.linkageBreaks.is_empty(),
        "an empty chain has nothing to break linkage: {:?}",
        status.linkageBreaks
    );
}

/// **The house standard.** Attempts a real write against `AuditAnchor`
/// through the real generated delegate, under the most privileged context
/// this codebase ever constructs (`system`), on a row id that does not
/// exist — so a `NotFound` would mean the *policy* check never ran (the
/// actual hole this test exists to rule out), and only a `Forbidden` means
/// the policy itself refused the write before ever looking for the row.
/// Captured for real, once, while writing this file, with `sys()`:
///
/// ```text
/// Err(Forbidden("update policy denied this operation"))
/// ```
///
/// No `@@allow("update", ...)` / `@@allow("delete", ...)` clause exists on
/// `AuditAnchor` in `schema.cstack` — deny-by-default (§2.0) is what
/// produces this, for every role, forever, unless someone adds one.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres — see module docs"]
async fn no_role_including_system_can_write_an_audit_anchor() {
    let _guard = TEST_MUTEX.lock().await;
    let db = db().await;

    let update_result = db
        .audit_anchor()
        .update("this-id-does-not-exist".to_owned())
        .set(UpdateAuditAnchorInput {
            rowCount: Some(999),
            ..Default::default()
        })
        .run(&sys())
        .await;
    assert!(
        matches!(update_result, Err(CoolError::Forbidden(_))),
        "expected Forbidden (policy denial), not NotFound (which would mean the policy check \
         never ran) or Ok (which would mean AuditAnchor is writable): got {update_result:?}"
    );

    // AuditAnchor's own schema.cstack model declares no @@allow("delete",
    // ...) clause either — same deny-by-default mechanism, checked
    // directly rather than assumed to follow from the update case alone.
    let delete_result = db
        .audit_anchor()
        .delete("this-id-does-not-exist".to_owned())
        .run(&sys())
        .await;
    assert!(
        matches!(delete_result, Err(CoolError::Forbidden(_))),
        "expected Forbidden, got {delete_result:?}"
    );
}
