//! Live, `#[ignore]`d verification against a real `sms-gateway`. Not part of
//! any CI job (this crate has no live-stack wiring - see `examples/rust`'s own
//! documented CI gap for the same shape of gap). Run by hand:
//!
//! ```bash
//! # 1. A gateway, started with its two envelope key files, and a database:
//! #      sms-gateway serve --envelope-signing-key-file current.pem \
//! #                        --envelope-next-signing-key-file next.pem ...
//! # 2. A signing key whose public half is registered for an AppClient:
//! #      openssl genpkey -algorithm ed25519 -out sdk-test-key.pem
//! #      sms-gateway register-client --app <slug> --scope sms:send --scope sms:read \
//! #        --public-key "$(cat <the key's base64url public half>)"
//! # 3. The gateway's pinned keys:
//! #      sms-gateway server-keys --envelope-signing-key-file current.pem \
//! #                              --envelope-next-signing-key-file next.pem > server-keys.txt
//! cd sdks/rust
//! VSMS_SDK_TEST_SIGNING_KEY_PATH=sdk-test-key.pem VSMS_SDK_TEST_SERVER_KEYS_PATH=server-keys.txt \
//!   cargo test -p vsms-sdk-rust --test live_gateway -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `VSMS_SDK_TEST_BASE_URL` (default `http://127.0.0.1:8080`) and
//! `DATABASE_URL` (default the demo stack's own
//! `postgres://postgres:postgres@localhost:15433/vsms_demo`) can override the
//! rest. The two key paths have no defaults: they are per-run material, so the
//! tests panic with a message pointing back here if one is missing.
//!
//! These three tests are this SDK's own version of issue #171's measurable
//! acceptance criteria, restated for the signed transport: "N sends are N
//! signed requests, none of them minting or refreshing anything" (replacing
//! "token reuse"), "a refused signature is surfaced after exactly one attempt"
//! (replacing "a deliberate 401 triggers exactly one refresh"), and - added once
//! #153 (`IdempotencyLayer`) landed - "a replayed `Idempotency-Key` does not
//! create a second `Message` row". A live run reaching `delivered` is proven
//! separately by `examples/rust/sms-send` against the same stack.
//! `--test-threads=1` matters: these tests observe shared state (Postgres)
//! that would race under parallel execution.

use std::process::Command;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use vsms_sdk::schema::SendMessageInput;
use vsms_sdk::{ServerKeys, SigningKey, VsmsClient};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn required(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        panic!("{key} is required for this live test - see this test file's own module doc")
    })
}

fn default_database_url() -> String {
    // Matches the demo stack's own DATABASE_URL.
    "postgres://postgres:postgres@localhost:15433/vsms_demo".to_owned()
}

/// A client signing with `VSMS_SDK_TEST_SIGNING_KEY_PATH`'s key against the
/// gateway at `VSMS_SDK_TEST_BASE_URL`, pinning
/// `VSMS_SDK_TEST_SERVER_KEYS_PATH`'s keys.
fn live_client() -> (VsmsClient, SigningKey) {
    let base_url = env_or("VSMS_SDK_TEST_BASE_URL", "http://127.0.0.1:8080");
    let key = SigningKey::from_pem_file(required("VSMS_SDK_TEST_SIGNING_KEY_PATH"))
        .expect("reading the signing key should succeed");
    let report = std::fs::read_to_string(required("VSMS_SDK_TEST_SERVER_KEYS_PATH"))
        .expect("reading the server keys file should succeed");
    let server_keys = ServerKeys::parse_report(&report).expect("a valid `server-keys` report");
    let client = VsmsClient::signed(base_url, key.clone(), server_keys)
        .expect("building a VsmsClient against a reachable gateway should succeed");
    (client, key)
}

/// Shells out to `psql` rather than adding a Postgres client crate as a
/// dependency just for this test file - `psql` is already the tool every
/// script in the main vsms repo uses for exactly this kind of by-hand
/// verification. `sql` must be a query returning a single row, single
/// plain-integer column (`-tAc`, no header, no alignment).
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

/// The rows in `envelope_nonces` recorded for `key`: the replay cache holds one
/// row per accepted request, named `cose:<kid>:<cti>`, where `kid` is the first
/// 8 bytes of the key's RFC 9679 thumbprint. Counting them for one key's `kid`
/// is the gateway's own record of how many signed requests that key made, with
/// no noise from other principals.
fn count_nonces_for(database_url: &str, key: &SigningKey) -> usize {
    let public = URL_SAFE_NO_PAD
        .decode(key.public_key_b64u())
        .expect("a base64url public key");
    let public: [u8; 32] = public.try_into().expect("32 bytes");
    let kid = cratestack::cose::CoseVerifyKey::ed25519(&public)
        .expect("a valid public key")
        .kid();
    let kid_hex: String = kid.iter().map(|byte| format!("{byte:02x}")).collect();
    count_rows(
        database_url,
        &format!("SELECT count(*) FROM envelope_nonces WHERE nonce LIKE 'cose:{kid_hex}:%'"),
    )
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

/// Issue #171, restated: N sends are N signed requests and nothing else.
/// Sends three real messages through one `VsmsClient`, each with a distinct
/// `clientRef` (so none collide on the dedupe index), and asserts the gateway
/// recorded exactly three nonces for this key - one per request, each of them a
/// fresh, never-reused `cti`. There is no token to mint, cache or refresh, so
/// there is nothing else to count.
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn n_sends_are_n_signed_requests_live() {
    let database_url = env_or("DATABASE_URL", &default_database_url());
    let (client, key) = live_client();

    let before = count_nonces_for(&database_url, &key);

    for i in 0..3 {
        let args = SendMessageInput {
            to: "+237677123456".to_owned(),
            body: format!("vsms-sdk-rust signed-request verification #{i}"),
            senderId: Some("VYMALO".to_owned()),
            class: None,
            clientRef: Some(format!("vsms-sdk-rust-signed-{}-{i}", std::process::id())),
            scheduledAt: None,
            validityMinutes: None,
        };
        client
            .send_message(args, None)
            .await
            .unwrap_or_else(|error| panic!("send #{i} should succeed: {error}"));
    }

    let after = count_nonces_for(&database_url, &key);
    assert_eq!(
        after - before,
        3,
        "3 sends should record exactly 3 nonces for this key (before={before}, after={after})"
    );
}

/// Issue #171: `Idempotency-Key` support, added once #153 (which mounted
/// `IdempotencyLayer` on the generated router) landed. Sends the exact
/// same `SendMessageInput` twice under one `Idempotency-Key` and proves,
/// by counting `messages` rows in the database - not by trusting
/// `idempotency_replayed`, which is only an echo of an unauthenticated
/// response header - that the second call created no second row:
/// `after - before == 1`, not 2. Each call is a freshly sealed request (its own
/// nonce), which is the point of the layering: the idempotency layer sits
/// inside the envelope, so a re-sealed retry still replays. Also checks the
/// header echo agrees with that ground truth (`false` then `true`) and that
/// both calls report the identical `messageId`.
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn idempotency_key_replay_does_not_duplicate_the_message_live() {
    let database_url = env_or("DATABASE_URL", &default_database_url());
    let (client, _key) = live_client();

    // A unique marker per run so `count_messages_with_body` can never
    // match a row left over from an earlier run of this same test.
    let marker = format!(
        "vsms-sdk-rust idempotency-replay verification pid={}",
        std::process::id()
    );
    let idempotency_key = format!("vsms-sdk-rust-idem-{}", std::process::id());
    // No `clientRef`: replay must work off `Idempotency-Key` alone, and a
    // replay requires the *exact same* request body on both calls - a
    // `clientRef` would also mean the database-level dedupe
    // (`messages_app_idem_key`) could be what's actually preventing a second
    // row, muddying which layer this test is proving.
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

/// Issue #171, restated: "a refused signature is surfaced after exactly one
/// attempt." A key nobody registered is refused by the gateway with its coarse
/// unsigned 401 (an unknown key is indistinguishable from a forgery on
/// purpose), and the SDK returns that to the caller without retrying - there
/// is no token to refresh, and a retry is a new request with a new nonce, the
/// caller's decision. The one-attempt half is proven without a gateway by
/// `tests/transport.rs`; this is the half only a real gateway can answer: that
/// it really does say 401, unsigned, to an unregistered key.
#[tokio::test]
#[ignore = "needs a live sms-gateway; see this file's own module doc"]
async fn an_unregistered_key_is_refused_unsigned_live() {
    let base_url = env_or("VSMS_SDK_TEST_BASE_URL", "http://127.0.0.1:8080");
    let report = std::fs::read_to_string(required("VSMS_SDK_TEST_SERVER_KEYS_PATH"))
        .expect("reading the server keys file should succeed");
    let client = VsmsClient::signed(
        base_url,
        SigningKey::generate(),
        ServerKeys::parse_report(&report).expect("a valid `server-keys` report"),
    )
    .expect("building a VsmsClient should succeed");

    let args = SendMessageInput {
        to: "+237677123456".to_owned(),
        body: "vsms-sdk-rust unregistered-key verification".to_owned(),
        senderId: Some("VYMALO".to_owned()),
        class: None,
        clientRef: Some(format!("vsms-sdk-rust-unregistered-{}", std::process::id())),
        scheduledAt: None,
        validityMinutes: None,
    };
    let result = client.send_message(args, None).await;

    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(vsms_sdk::SdkError::is_unauthorized),
        "expected a 401 to surface, got: {result:?}"
    );
}
