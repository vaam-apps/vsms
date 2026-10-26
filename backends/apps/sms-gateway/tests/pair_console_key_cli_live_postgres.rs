//! `sms-gateway pair-console-key` (VS-5a) as a real, separate OS process
//! against a real Postgres: the break-glass half of `registerConsoleKey`.
//!
//! Two fast tests cover input the command refuses before it touches the
//! database; the live ones prove the claims that matter - running it twice
//! leaves one key and exits 0, it never creates a user and never changes a
//! role (an owner stays an owner, an admin an admin), a deactivated or unknown
//! user is refused with nothing written, and a revoked key is never
//! reinstated.
//!
//! ```bash
//! cargo test -p sms-gateway --test pair_console_key_cli_live_postgres -- --ignored --nocapture
//! ```

use std::process::{Command, Output};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Duration;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CratestackContext, FilterExpr};
use p256::ecdsa::SigningKey;
use sha2::{Digest, Sha256};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{self, Cratestack, console_key, user};

/// See `provision_client_cli_live_postgres.rs`'s identical mutex (#102).
static TEST_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn context(role: &str) -> CratestackContext {
    Principal {
        sub: format!("pair-console-key-cli-test-{role}"),
        kind: if role == "system" {
            PrincipalKind::App
        } else {
            PrincipalKind::User
        },
        role: role.to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .subsec_nanos();
    format!("{:06x}", (u64::from(nanos).wrapping_add(n)) % 0x0100_0000)
}

/// A distinct P-256 public key per call (SEC1 uncompressed, base64url). The
/// scalar is hashed from a counter, the process id and the clock; the private
/// half is dropped on the spot, as it would be on the machine that owns it.
/// Note an uncompressed point starts `0x04`, which base64url spells `B`, so a
/// key here never begins with `-`; the hyphen case is a unit test in the
/// binary, not something to hope a random key hits.
fn fresh_public_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    let mut seed: [u8; 32] =
        Sha256::digest(format!("pair-cli-{n}-{}-{nanos}", std::process::id())).into();
    let key = loop {
        if let Ok(key) = SigningKey::from_slice(&seed) {
            break key;
        }
        seed = Sha256::digest(seed).into();
    };
    URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes())
}

async fn db() -> (Cratestack, String) {
    let url = sms_test_support::database_url().await;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connecting to Postgres");
    (Cratestack::builder(pool).build(), url)
}

async fn seed_user(db: &Cratestack, role_key: &str) -> schema::User {
    db.user()
        .create(schema::CreateUserInput {
            subject: format!("pending-{}", unique_suffix()),
            email: format!("cli-{}@example.test", unique_suffix()),
            displayName: "Break Glass".to_owned(),
            roleKey: role_key.to_owned(),
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&context("owner"))
        .await
        .expect("seeding the user")
}

fn run(database_url: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sms-gateway"))
        .arg("pair-console-key")
        .arg("--database-url")
        .arg(database_url)
        .args(extra)
        .output()
        .expect("running `sms-gateway pair-console-key`")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A `key: value` line of the command's documented stdout.
fn field(stdout: &str, name: &str) -> String {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("expected a \"{name}: ...\" line in stdout, got: {stdout:?}"))
        .trim()
        .to_owned()
}

async fn keys_of(db: &Cratestack, user_id: &str) -> Vec<schema::ConsoleKey> {
    db.console_key()
        .find_many()
        .where_expr(FilterExpr::from(
            console_key::userId().eq(user_id.to_owned()),
        ))
        .run(&context("system"))
        .await
        .expect("reading console keys back")
}

async fn user_count(db: &Cratestack) -> usize {
    db.user()
        .find_many()
        .where_expr(FilterExpr::from(user::email().contains("@".to_owned())))
        .limit(100_000)
        .run(&context("system"))
        .await
        .expect("counting users")
        .len()
}

#[test]
fn a_malformed_public_key_is_refused_before_the_database_is_touched() {
    let good = fresh_public_key();
    let decoded = URL_SAFE_NO_PAD.decode(&good).expect("base64url");
    let compressed = {
        let mut point = decoded.clone();
        point[0] = 0x02;
        URL_SAFE_NO_PAD.encode(&point[..33])
    };
    for bad in [
        String::new(),
        "not-a-key".to_owned(),
        "AAAA".to_owned(),
        // An Ed25519-sized key, and a compressed P-256 point.
        URL_SAFE_NO_PAD.encode([9_u8; 32]),
        compressed,
    ] {
        let output = run(
            "postgres://never-contacted.invalid/vsms",
            &["--user", "someone@example.test", "--public-key", &bad],
        );
        assert!(!output.status.success(), "{bad:?} must be refused");
        assert!(
            stderr(&output).contains("--public-key"),
            "{bad:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn an_empty_user_is_refused_before_the_database_is_touched() {
    let output = run(
        "postgres://never-contacted.invalid/vsms",
        &["--user", "  ", "--public-key", &fresh_public_key()],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--user"), "{}", stderr(&output));
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn running_it_twice_leaves_one_key_and_exits_zero_both_times() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let seeded = seed_user(&db, "admin").await;
    let key = fresh_public_key();
    let args = [
        "--user",
        &seeded.email,
        "--public-key",
        &key,
        "--label",
        "break-glass",
    ];

    let first = run(&url, &args);
    assert!(first.status.success(), "first run: {}", stderr(&first));
    let second = run(&url, &args);
    assert!(second.status.success(), "second run: {}", stderr(&second));

    let (first_out, second_out) = (stdout(&first), stdout(&second));
    assert_eq!(field(&first_out, "key id"), field(&second_out, "key id"));
    assert_eq!(
        field(&first_out, "thumbprint"),
        field(&second_out, "thumbprint")
    );
    assert_eq!(field(&first_out, "thumbprint").len(), 64);
    assert!(first_out.contains("registered"), "{first_out}");
    assert!(second_out.contains("already registered"), "{second_out}");

    let keys = keys_of(&db, &seeded.id).await;
    assert_eq!(keys.len(), 1, "the same key twice is one row");
    assert_eq!(keys[0].id, field(&first_out, "key id"));
    assert_eq!(keys[0].label.as_deref(), Some("break-glass"));
    assert_eq!(keys[0].alg, schema::ConsoleKeyAlg::esp256);
    assert_eq!(
        keys[0].expiresAt - keys[0].pairedAt,
        Duration::days(30),
        "R3: the break-glass key lasts 30 days too"
    );
}

/// It never creates a user and never changes a role: an owner locked out gets
/// a key and is still the owner; an admin is still an admin; nobody is
/// promoted. By id as well as by email.
#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn it_changes_no_role_and_creates_no_user() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let owner = seed_user(&db, "owner").await;
    let support = seed_user(&db, "support").await;
    let before = user_count(&db).await;

    for (seeded, selector) in [
        (&owner, owner.email.clone()),
        (&support, support.id.clone()),
    ] {
        let output = run(
            &url,
            &["--user", &selector, "--public-key", &fresh_public_key()],
        );
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(
            stdout(&output).contains(&format!("role={}", seeded.roleKey)),
            "{}",
            stdout(&output)
        );
        assert_eq!(keys_of(&db, &seeded.id).await.len(), 1);
    }

    assert_eq!(user_count(&db).await, before, "no user was created");
    let after = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(
            user::id().in_(vec![owner.id.clone(), support.id.clone()]),
        ))
        .run(&context("system"))
        .await
        .expect("reading the users back");
    for row in after {
        let was = if row.id == owner.id {
            "owner"
        } else {
            "support"
        };
        assert_eq!(row.roleKey, was, "no role changed");
        assert_eq!(
            row.version,
            if row.id == owner.id {
                owner.version
            } else {
                support.version
            }
        );
    }
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn an_unknown_or_deactivated_user_is_refused_and_nothing_is_written() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let key = fresh_public_key();

    let unknown = run(
        &url,
        &["--user", "nobody-here@example.test", "--public-key", &key],
    );
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("no active user"),
        "{}",
        stderr(&unknown)
    );
    let unknown_id = run(
        &url,
        &["--user", "nosuchuser0000000000000", "--public-key", &key],
    );
    assert!(!unknown_id.status.success());

    let seeded = seed_user(&db, "admin").await;
    db.user()
        .update(seeded.id.clone())
        .set(schema::UpdateUserInput {
            active: Some(false),
            ..Default::default()
        })
        .if_match(seeded.version)
        .run(&context("owner"))
        .await
        .expect("deactivating the user");
    let deactivated = run(&url, &["--user", &seeded.email, "--public-key", &key]);
    assert!(!deactivated.status.success());
    assert!(
        stderr(&deactivated).contains("no active user"),
        "{}",
        stderr(&deactivated)
    );
    assert_eq!(keys_of(&db, &seeded.id).await.len(), 0);
}

#[tokio::test]
#[ignore = "needs a live, migrated Postgres - see module docs"]
async fn a_revoked_key_or_another_users_key_is_refused_with_a_non_zero_exit() {
    let _guard = TEST_MUTEX.lock().await;
    let (db, url) = db().await;
    let (ada, grace) = (seed_user(&db, "admin").await, seed_user(&db, "admin").await);
    let key = fresh_public_key();
    let first = run(&url, &["--user", &ada.email, "--public-key", &key]);
    assert!(first.status.success(), "{}", stderr(&first));

    let stolen = run(&url, &["--user", &grace.email, "--public-key", &key]);
    assert!(
        !stolen.status.success(),
        "a key cannot move to another user"
    );
    assert!(
        stderr(&stolen).contains("another user"),
        "{}",
        stderr(&stolen)
    );
    assert_eq!(keys_of(&db, &grace.id).await.len(), 0);

    // Revocation is the procedure's, not the CLI's; stamp it the way
    // `revokeConsoleKey` does (system-only column).
    db.console_key()
        .update(field(&stdout(&first), "key id"))
        .set(schema::UpdateConsoleKeyInput {
            revokedAt: Some(Some(chrono::Utc::now())),
            ..Default::default()
        })
        .run(&context("system"))
        .await
        .expect("revoking as system");
    let again = run(&url, &["--user", &ada.email, "--public-key", &key]);
    assert!(!again.status.success(), "a revoked key is never reinstated");
    assert!(stderr(&again).contains("revoked"), "{}", stderr(&again));
}
