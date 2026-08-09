//! Live, `#[ignore]`d verification against a real `sms-gateway` — the
//! `just demo` stack, run from the main vsms repo root. Not part of any
//! CI job yet (this crate has no CI wiring at all — see its own module
//! doc's "measured build cost" section and `examples/rust`'s own
//! documented CI gap for the same shape of gap). Run by hand:
//!
//! ```bash
//! just demo   # from the repo root; prints a client id and writes a key
//! cd sdks/rust
//! VSMS_SDK_TEST_CLIENT_ID=<clientId just demo printed> \
//!   cargo test -p vsms-sdk-rust --test live_gateway -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `VSMS_SDK_TEST_ISSUER` (default `http://127.0.0.1:8080`),
//! `VSMS_SDK_TEST_PRIVATE_KEY_PATH` (default `<repo root>/.demo/
//! console-client-key.pem`, `just demo`'s own conventional location), and
//! `DATABASE_URL` (default `just demo`'s own
//! `postgres://postgres:postgres@localhost:15433/vsms_demo`) can override
//! the rest. `VSMS_SDK_TEST_CLIENT_ID` has no sensible default — `just
//! demo` mints a fresh `App`/client id on every run — so it must be
//! passed explicitly; both tests panic with a clear message pointing back
//! to this doc comment if it's missing.
//!
//! These three tests are this SDK's own version of issue #171's
//! measurable acceptance criteria: "token reuse demonstrated: N sends do
//! not mint N tokens," "a deliberate 401 triggers exactly one refresh,
//! then surfaces the error," and — added once #153 (`IdempotencyLayer`)
//! landed — "a replayed `Idempotency-Key` does not create a second
//! `Message` row." A live run reaching `delivered` is proven separately
//! by `examples/rust/sms-send` against this same stack (see its own
//! README for the verification table); nothing here re-proves that part.
//! `--test-threads=1` matters: these tests observe shared state
//! (Postgres, in two of the three) that would race under parallel
//! execution — the same reason the main vsms repo's own live suites all
//! take a `tokio::sync::Mutex` (see `AGENTS.md`'s "live suites run in
//! CI" note).

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{PrivateKeyJwtConfig, SdkError, TokenStore, VsmsClient};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn required_client_id() -> String {
    std::env::var("VSMS_SDK_TEST_CLIENT_ID").unwrap_or_else(|_| {
        panic!(
            "VSMS_SDK_TEST_CLIENT_ID is required for this live test — run `just demo` from the \
             repo root, then pass the client id it prints (\"provisioned client: <id>\"); see \
             this test file's own module doc"
        )
    })
}

fn default_key_path() -> String {
    // CARGO_MANIFEST_DIR is sdks/rust/vsms-sdk-rust; `just demo`'s own
    // conventional key location is <repo root>/.demo/console-client-key.pem.
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../.demo/console-client-key.pem"
    )
    .to_owned()
}

fn default_database_url() -> String {
    // Matches scripts/demo.sh's own DATABASE_URL exactly.
    "postgres://postgres:postgres@localhost:15433/vsms_demo".to_owned()
}

/// The total row count in `client_assertions` — the table `authkestra-op`
/// inserts into exactly once per successful `/token` exchange (it's how
/// `record_jti`'s replay protection works — see `AGENTS.md`'s M1 section
/// and this crate's own `token.rs` module doc). The table has no
/// `client_id` column (it's a global replay-protection ledger, not
/// scoped per client — confirmed with `\d client_assertions` against a
/// real `just demo` database), so this is a whole-table count, not one
/// scoped to the client under test. That's a real, stated limitation for
/// this specific test: a concurrent `/token` exchange from something else
/// against the same database during the test window would inflate the
/// delta this test measures. Acceptable for a manual, by-hand
/// verification run against a private `just demo` instance (nothing else
/// in that stack calls `/token` on its own), not something this test
/// tries to paper over as airtight in general.
///
/// Shells out to `psql` rather than adding a Postgres client crate as a
/// dependency just for this test file — `psql` is already the tool every
/// script in the main vsms repo uses for exactly this kind of by-hand
/// verification (`ci/apply-migrations.sh`, `justfile`'s `schema-check`,
/// ...). `sql` must be a query returning a single row, single plain-integer
/// column (`-tAc`, no header, no alignment).
fn count_rows(database_url: &str, sql: &str) -> usize {
    let output = Command::new("psql")
        .arg(database_url)
        .args(["-tAc", sql])
        .output()
        .expect("running psql should succeed — is psql on PATH and is the demo Postgres up?");
    assert!(
        output.status.success(),
        "psql failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|error| {
            panic!(
                "expected a plain integer from psql, got {:?}: {error}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
}

/// The total row count in `client_assertions` — see this function's own
/// former doc, now folded into [`count_rows`]'s callers below: the table
/// `authkestra-op` inserts into exactly once per successful `/token`
/// exchange (`record_jti`'s replay protection — see `AGENTS.md`'s M1
/// section and this crate's own `token.rs` module doc). It has no
/// `client_id` column (a global replay-protection ledger, not scoped per
/// client — confirmed with `\d client_assertions` against a real `just
/// demo` database), so this is a whole-table count. A real, stated
/// limitation: a concurrent `/token` exchange from something else against
/// the same database during the test window would inflate the delta this
/// test measures — acceptable for a manual, by-hand verification run
/// against a private `just demo` instance (nothing else in that stack
/// calls `/token` on its own).
fn count_client_assertions(database_url: &str) -> usize {
    count_rows(database_url, "SELECT count(*) FROM client_assertions")
}

/// The row count in `messages` whose `body` exactly matches `body` — used
/// to prove an `Idempotency-Key` replay creates no second `Message` row,
/// by counting the database's own state rather than inferring it from
/// `idempotency_replayed` (a response-header echo, not proof the write
/// itself didn't happen twice — the two are independent claims, and this
/// test checks the one that actually matters). `body` is expected to
/// carry a caller-chosen unique marker (a pid, in the test below) so this
/// never matches a row from an unrelated run.
fn count_messages_with_body(database_url: &str, body: &str) -> usize {
    count_rows(
        database_url,
        &format!(
            "SELECT count(*) FROM messages WHERE body = {}",
            psql_quote(body)
        ),
    )
}

/// Minimal SQL string literal quoting — doubles embedded single quotes.
/// Fine for the fully caller-controlled, ASCII-only marker strings this
/// test file builds (`format!("...{}...", std::process::id())`), not
/// meant as a general-purpose escaper.
fn psql_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Issue #171: "token reuse demonstrated: N sends do not mint N tokens."
/// Sends three real messages through one `VsmsClient`, each with a
/// distinct `clientRef` (so none collide on the dedupe index and each is
/// a genuine, separate `sendMessage` call), and asserts the number of
/// real `/token` exchanges — read from the database's own replay-
/// protection ledger, not a client-side count that could itself be
/// wrong — increased by exactly 1, not 3.
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn n_sends_reuse_one_token_live() {
    let issuer = env_or("VSMS_SDK_TEST_ISSUER", "http://127.0.0.1:8080");
    let client_id = required_client_id();
    let key_path = env_or("VSMS_SDK_TEST_PRIVATE_KEY_PATH", &default_key_path());
    let database_url = env_or("DATABASE_URL", &default_database_url());

    let config =
        PrivateKeyJwtConfig::from_key_path(&issuer, &client_id, &key_path, "sms:send sms:read")
            .expect("reading the private key at VSMS_SDK_TEST_PRIVATE_KEY_PATH should succeed");
    let client = VsmsClient::private_key_jwt(&issuer, config)
        .expect("building a VsmsClient against a reachable gateway should succeed");

    let before = count_client_assertions(&database_url);

    for i in 0..3 {
        let args = SendMessageInput {
            to: "+237677123456".to_owned(),
            body: format!("vsms-sdk-rust token-reuse verification #{i}"),
            senderId: Some("VYMALO".to_owned()),
            class: None,
            clientRef: Some(format!(
                "vsms-sdk-rust-token-reuse-{}-{i}",
                std::process::id()
            )),
            scheduledAt: None,
            validityMinutes: None,
        };
        client
            .send_message(args, None)
            .await
            .unwrap_or_else(|error| panic!("send #{i} should succeed: {error}"));
    }

    let after = count_client_assertions(&database_url);
    assert_eq!(
        after - before,
        1,
        "3 sends through one VsmsClient should mint exactly 1 token (1 new client_assertions \
         row), not 3 — the cache in PrivateKeyJwtTokenStore::get_token isn't doing its job if \
         this fails (before={before}, after={after})"
    );
}

/// Issue #171: `Idempotency-Key` support, added once #153 (which mounted
/// `IdempotencyLayer` on the generated router) landed. Sends the exact
/// same `SendMessageInput` twice under one `Idempotency-Key` and proves,
/// by counting `messages` rows in the database — not by trusting
/// `idempotency_replayed`, which is only an echo of a response header —
/// that the second call created no second row: `after - before == 1`,
/// not 2. Also checks the header echo agrees with that ground truth
/// (`false` then `true`) and that both calls report the identical
/// `messageId`, since a replay is defined as "the same response," not
/// merely "no new row."
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn idempotency_key_replay_does_not_duplicate_the_message_live() {
    let issuer = env_or("VSMS_SDK_TEST_ISSUER", "http://127.0.0.1:8080");
    let client_id = required_client_id();
    let key_path = env_or("VSMS_SDK_TEST_PRIVATE_KEY_PATH", &default_key_path());
    let database_url = env_or("DATABASE_URL", &default_database_url());

    let config =
        PrivateKeyJwtConfig::from_key_path(&issuer, &client_id, &key_path, "sms:send sms:read")
            .expect("reading the private key at VSMS_SDK_TEST_PRIVATE_KEY_PATH should succeed");
    let client = VsmsClient::private_key_jwt(&issuer, config)
        .expect("building a VsmsClient against a reachable gateway should succeed");

    // A unique marker per run so `count_messages_with_body` can never
    // match a row left over from an earlier run of this same test.
    let marker = format!(
        "vsms-sdk-rust idempotency-replay verification pid={}",
        std::process::id()
    );
    let idempotency_key = format!("vsms-sdk-rust-idem-{}", std::process::id());
    // No `clientRef`: replay must work off `Idempotency-Key` alone, and a
    // replay requires the *exact same* request body on both calls — a
    // `clientRef` would make that trivially true but would also mean the
    // database-level dedupe (`messages_app_idem_key`) could be what's
    // actually preventing a second row, muddying which layer this test
    // is proving.
    let build_args = || SendMessageInput {
        to: "+237677123456".to_owned(),
        body: marker.clone(),
        senderId: Some("VYMALO".to_owned()),
        class: None,
        clientRef: None,
        scheduledAt: None,
        validityMinutes: None,
    };

    let before = count_messages_with_body(&database_url, &marker);

    let first = client
        .send_message(build_args(), Some(&idempotency_key))
        .await
        .expect("the first send under a fresh Idempotency-Key should succeed");
    assert!(
        !first.idempotency_replayed,
        "the first call under a never-before-used Idempotency-Key must not be a replay"
    );

    let second = client
        .send_message(build_args(), Some(&idempotency_key))
        .await
        .expect("the second send under the same Idempotency-Key should succeed (a replay, not an error)");
    assert!(
        second.idempotency_replayed,
        "the second call under the same Idempotency-Key + identical body must be a replay"
    );
    assert_eq!(
        first.result.messageId, second.result.messageId,
        "a replay must return the exact same messageId as the original call, not a new one"
    );

    let after = count_messages_with_body(&database_url, &marker);
    assert_eq!(
        after - before,
        1,
        "one Idempotency-Key-replayed retry must not create a second Message row \
         (before={before}, after={after})"
    );
}

/// A `TokenStore` double that always hands back a syntactically-plausible
/// but invalid token, so every real call the SDK makes with it gets a
/// genuine `401` from the real gateway. Counts how many times each trait
/// method was actually called — the only way to prove "exactly one
/// refresh, not a loop" from outside `VsmsClient` without reading its
/// source. Needs no `/token` exchange at all (it never delegates to a
/// real `PrivateKeyJwtTokenStore`), so unlike the test above this one
/// needs no database access — only the real gateway's real RBAC/JWT
/// validation rejecting a bogus Bearer token, twice.
#[derive(Default)]
struct AlwaysUnauthorizedTokenStore {
    get_token_calls: AtomicUsize,
    invalidate_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl TokenStore for AlwaysUnauthorizedTokenStore {
    async fn get_token(&self) -> Result<String, SdkError> {
        self.get_token_calls.fetch_add(1, Ordering::SeqCst);
        Ok("this-is-not-a-real-token".to_owned())
    }

    async fn invalidate(&self) {
        self.invalidate_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// Issue #171: "a deliberate 401 triggers exactly one refresh, then
/// surfaces the error." A token that's never valid should make
/// `VsmsClient::send_message` try exactly twice (the original attempt,
/// then one retry after `invalidate()`) and then return the `401` to the
/// caller — never loop, since `/token` has no rate limiting today (#156)
/// and an unbounded refresh loop against it would be a self-inflicted
/// denial of service.
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn deliberate_401_triggers_exactly_one_refresh_live() {
    let gateway_base_url = env_or("VSMS_SDK_TEST_ISSUER", "http://127.0.0.1:8080");

    let store = Arc::new(AlwaysUnauthorizedTokenStore::default());
    let client = VsmsClient::builder()
        .base_url(gateway_base_url)
        .token_store(store.clone())
        .build()
        .expect("building a VsmsClient with a hand-rolled TokenStore should succeed");

    let args = SendMessageInput {
        to: "+237677123456".to_owned(),
        body: "vsms-sdk-rust 401-bound verification".to_owned(),
        senderId: Some("VYMALO".to_owned()),
        class: None,
        clientRef: Some(format!("vsms-sdk-rust-401-bound-{}", std::process::id())),
        scheduledAt: None,
        validityMinutes: None,
    };
    let result = client.send_message(args, None).await;

    assert!(
        result.as_ref().err().is_some_and(SdkError::is_unauthorized),
        "expected a 401 to surface, got: {result:?}"
    );
    assert_eq!(
        store.get_token_calls.load(Ordering::SeqCst),
        2,
        "expected exactly 2 get_token calls (original + one bounded refresh), not a loop"
    );
    assert_eq!(
        store.invalidate_calls.load(Ordering::SeqCst),
        1,
        "expected exactly 1 invalidate call"
    );
}
