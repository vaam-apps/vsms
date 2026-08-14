//! Proves the one runtime path AGENTS.md flagged as unverified when
//! `authkestra-op`/`-engine`/`-resource`/`-axum` were switched to
//! `default-features = false, features = ["rustls-no-provider"]` (root
//! `Cargo.toml`) to drop `aws-lc-rs` out of the dependency graph: a
//! `reqwest` client built from that exact feature combination — no crypto
//! backend compiled in at all — completes a real TLS handshake once
//! `rustls::crypto::ring::default_provider().install_default()` has run,
//! rather than panicking on its first request. `backends/apps/sms-gateway/src/
//! main.rs`'s `install_default_crypto_provider` (and
//! `backends/apps/sms-worker/src/main.rs`'s identical copy) make that call the very
//! first thing either binary's `main` does, for exactly this reason.
//!
//! `#[ignore]`d for the same reason as this crate's `_live_postgres`
//! suites — a "live" test outside the default `cargo test --workspace`
//! gate — but for a different dependency: this one needs outbound HTTPS
//! reachability, not a Postgres container. Run directly (`cargo test -p
//! sms-gateway --test tls_no_provider_live -- --ignored`) or via `just
//! test-live`, which also happens to start the Postgres harness the other
//! ignored suites in this workspace need — harmless, just unused here.
//!
//! Deliberately a real network call, not a `wiremock` fixture — `wiremock`
//! only ever terminates plain HTTP (this repo's own chaos-suite doc,
//! `AGENTS.md`, already notes TLS-level behaviour is out of its reach), so
//! it structurally cannot exercise a TLS handshake at all. `https://
//! api.orange.com` isn't a stand-in chosen at random: it's the exact host
//! `ORANGE_CM_BASE_URL` defaults to in `backends/apps/sms-gateway/src/main.rs` and
//! `backends/apps/sms-worker/src/main.rs`, so this test's request is a real instance
//! of the kind of outbound call these binaries already make in
//! production. Only the TLS handshake and HTTP round trip are asserted —
//! the response status is irrelevant; an unauthenticated, pathless GET
//! against Orange's real API is expected to be rejected, not answered
//! `200`.
//!
//! `reqwest-no-provider` (this crate's own `Cargo.toml`) is a second,
//! independently-featured resolution of the identical `reqwest 0.13.4`
//! already in the workspace graph via authkestra's own pin — not a
//! synthetic reproduction. `cratestack-client-rust`'s own test suite
//! (`error.rs`, found while investigating this exact mechanism) installs
//! the provider the same way for the same reason, and
//! `cratestack-client-rust`'s own `CratestackClient::new` calls the
//! identical `rustls::crypto::ring::default_provider().install_default()`
//! before building its own client — independent, upstream confirmation
//! this is the correct fix, not just a workaround invented here.

/// Mirrors `install_default_crypto_provider` in both binaries' `main.rs`
/// exactly — including the `.ok()`-equivalent `let _ =`, since a provider
/// already installed (e.g. by a prior test in the same process) is not a
/// failure.
fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::test]
#[ignore = "needs outbound HTTPS reachability, not this workspace's Postgres harness"]
async fn a_provider_installed_before_client_construction_lets_rustls_no_provider_reqwest_complete_a_real_handshake(
) {
    install_default_crypto_provider();

    let client = reqwest_no_provider::Client::builder().build().expect(
        "building a reqwest client after installing a default CryptoProvider must not fail — a \
         failure here would mean the ordering this test exists to prove is already broken before \
         a single request is even sent",
    );

    let response = client.get("https://api.orange.com/").send().await.expect(
        "a real HTTPS request through a rustls-no-provider reqwest client must complete once a \
         default CryptoProvider has been installed — a panic or TLS-level connection error here \
         is exactly the unverified failure mode AGENTS.md's own \"aws-lc-rs enters this tree\" \
         section warned about",
    );

    // Any HTTP response at all — even a 404/401 from an unauthenticated,
    // pathless request — is proof the TLS handshake and HTTP exchange both
    // completed. That's the only claim in question here.
    let _ = response.status();
}
