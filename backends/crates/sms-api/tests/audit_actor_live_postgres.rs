//! Whose `sub` the audit trail records as the actor.
//!
//! `@@audit` writes `cratestack_audit` in the same transaction as the mutation,
//! and its `actor` column carries an `id`. cratestack decides what that id is:
//! through 0.16 it read an `id` claim only, which this gateway's `auth
//! Principal` does not declare (it declares `sub`), so every audit row vsms
//! wrote had `actor->>'id'` null. From 0.17.0 the id is the principal
//! contract's `subject()` (`sub`, then `actor.id`, then `id`), so the row names
//! the caller. `audit_log::AuditLogFilter::actor_id` filters on that column.
//!
//! This file pins the behaviour at the data layer, against a real database,
//! because the unit test upstream builds its context by hand and cannot see
//! whether the gateway's own `Principal` projection feeds it.
//!
//! Ignored by default like the other live suites:
//!
//! ```bash
//! cargo test -p sms-api --test audit_actor_live_postgres -- --ignored
//! ```

use cratestack::CratestackContext;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack};

async fn db() -> Cratestack {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    Cratestack::builder(pool).build()
}

fn owner(sub: &str) -> CratestackContext {
    Principal {
        sub: sub.to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn suffix() -> String {
    cratestack::uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// `psql` against the harness database, for the one table the typed client does
/// not model: the framework's own `cratestack_audit`.
async fn audit_actor_ids(user_id: &str) -> Vec<String> {
    let database_url = sms_test_support::database_url().await;
    let output = std::process::Command::new("psql")
        .arg(database_url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-At", "-c"])
        .arg(format!(
            "SELECT coalesce(actor->>'id', '<null>') FROM cratestack_audit \
             WHERE model = 'User' AND primary_key::text LIKE '%{user_id}%' \
             ORDER BY occurred_at"
        ))
        .output()
        .expect("spawning `psql`");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
#[ignore = "needs a live Postgres (sms-test-support)"]
async fn an_audited_write_records_the_callers_sub_as_the_actor() {
    let db = db().await;
    let s = suffix();
    let caller = format!("audit-actor-test-owner-{s}");

    let user = db
        .user()
        .create(schema::CreateUserInput {
            subject: format!("audit-actor-{s}"),
            email: format!("audit-actor-{s}@example.test"),
            displayName: format!("audit actor {s}"),
            roleKey: "support".to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&owner(&caller))
        .await
        .expect("an owner creates a user");

    assert_eq!(
        audit_actor_ids(&user.id).await,
        vec![caller],
        "the audit row for the create names the caller's `sub` as the actor; `<null>` means \
         cratestack read no identity out of the gateway's principal"
    );
}
