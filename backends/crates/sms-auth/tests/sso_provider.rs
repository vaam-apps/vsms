//! The SSO broker's upstream half, against a scripted fake `IdP`
//! (`sms-fake-oidc`) over real HTTP: discovery, the JWKS, the authorization
//! URL, the `client_secret_basic` code exchange, and `id_token`
//! verification with tokens that are *actually* malformed - not helpers
//! asserted in isolation.
//!
//! No database: nothing here touches Postgres, so unlike the live suites
//! these run in the ordinary `cargo test --workspace`.

use sms_auth::sso::{AuthnSecrets, IdClaims, Refusal, SsoConfig, SsoError, SsoProvider};
use sms_fake_oidc::{FakeOidc, KidMode, LoginSpec, NonceMode, SigAlg, SigningKey};

const CLIENT_ID: &str = "vsms-test";
/// Drawn at run time, with characters that must be form-encoded in the Basic
/// header (`+`, `/`, `=`): no literal secret lives in the test source.
static CLIENT_SECRET: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{}+with/odd=chars",
        sms_core::password::generate_password(16)
    )
});

fn config_for(fake: &FakeOidc, secret: &str, min_refresh_secs: u64) -> SsoConfig {
    let block: sms_config::Sso = serde_json::from_value(serde_json::json!({
        "issuer": fake.issuer(),
        "client_id": CLIENT_ID,
        "client_secret": secret,
        "scopes": ["openid", "email", "profile"],
        "required_group": "vsms-staff",
        "groups_claim": "groups",
        "accepted_amr": ["otp"],
        "accepted_acr": [],
        "jit_role": "support",
        "jwks_min_refresh_secs": min_refresh_secs,
        "rate_limit": { "burst": 30, "refill_per_second": 1.0 },
    }))
    .expect("a complete block");
    SsoConfig::new(&block, "http://127.0.0.1:1", false).expect("a valid config")
}

async fn setup() -> (FakeOidc, SsoProvider) {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(&fake, &CLIENT_SECRET, 0)).expect("provider");
    (fake, provider)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

/// Plays the browser against the fake: follows the authorization URL one hop
/// and returns the `(code, state)` the fake redirected back with.
async fn authorize(
    provider: &SsoProvider,
    secrets: &AuthnSecrets,
    prompt_login: bool,
) -> (String, String) {
    let url = provider
        .authorization_url(secrets, prompt_login)
        .await
        .expect("authorization url");
    let response = http().get(url).send().await.expect("authorize");
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    let location = response.headers()["location"].to_str().expect("location");
    let location = url::Url::parse(location).expect("a url");
    let pairs: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    (
        pairs.get("code").cloned().expect("a code"),
        pairs.get("state").cloned().expect("a state"),
    )
}

async fn login(
    fake: &FakeOidc,
    provider: &SsoProvider,
    spec: LoginSpec,
) -> Result<IdClaims, SsoError> {
    fake.queue_login(spec);
    let secrets = AuthnSecrets::generate();
    let (code, state) = authorize(provider, &secrets, false).await;
    assert_eq!(state, secrets.state, "the IdP must echo the state");
    let raw = provider
        .exchange_code(&code, &secrets.code_verifier)
        .await?;
    provider.verify_id_token(&raw, &secrets.nonce).await
}

fn refused(result: Result<IdClaims, SsoError>) -> Refusal {
    match result {
        Err(SsoError::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_well_formed_token_verifies_and_yields_the_claims() {
    let (fake, provider) = setup().await;
    let claims = login(&fake, &provider, LoginSpec::default())
        .await
        .expect("verifies");
    assert_eq!(claims.subject, "fake-sub-1");
    assert_eq!(claims.email.as_deref(), Some("sso.user@example.test"));
    assert_eq!(claims.amr, vec!["pwd", "otp"]);
    assert_eq!(claims.groups, vec!["vsms-staff"]);
    assert_eq!(claims.email_verified, Some(true));
}

/// Only the JSON boolean `true` is the identity provider vouching for the
/// address: `"true"` the string (Apple's habit), a number, an absent claim and
/// `false` are all "not verified", and an account is never created on them.
#[tokio::test]
async fn email_verified_is_read_only_as_a_json_boolean() {
    let (fake, provider) = setup().await;
    let read = |spec: LoginSpec| {
        let (fake, provider) = (&fake, &provider);
        async move {
            login(fake, provider, spec)
                .await
                .expect("verifies")
                .email_verified
        }
    };
    let mut string = LoginSpec {
        email_verified: None,
        ..LoginSpec::default()
    };
    string.extra.insert("email_verified".into(), "true".into());
    assert_eq!(read(string).await, None, "a string is not a boolean");
    let absent = LoginSpec {
        email_verified: None,
        ..LoginSpec::default()
    };
    assert_eq!(read(absent).await, None);
    let false_ = LoginSpec {
        email_verified: Some(false),
        ..LoginSpec::default()
    };
    assert_eq!(read(false_).await, Some(false));
}

#[tokio::test]
async fn the_code_exchange_authenticates_with_basic_pkce_and_never_puts_the_secret_in_the_body() {
    let (fake, provider) = setup().await;
    login(&fake, &provider, LoginSpec::default())
        .await
        .expect("verifies");
    let requests = fake.token_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.client_authenticated, "client_secret_basic matched");
    assert!(
        request
            .authorization
            .as_deref()
            .is_some_and(|h| h.starts_with("Basic "))
    );
    assert!(!request.secret_in_body);
    assert!(request.redirect_uri_matched);
    assert!(
        request.verifier_matched,
        "the S256 verifier hashed to the challenge"
    );
    assert!(request.issued);
}

#[tokio::test]
async fn the_authorization_request_carries_pkce_state_nonce_and_prompt_only_on_retry() {
    let (fake, provider) = setup().await;
    let secrets = AuthnSecrets::generate();
    authorize(&provider, &secrets, false).await;
    authorize(&provider, &AuthnSecrets::generate(), true).await;
    let log = fake.authorize_requests();
    assert_eq!(log[0]["response_type"], "code");
    assert_eq!(log[0]["client_id"], CLIENT_ID);
    assert_eq!(log[0]["code_challenge_method"], "S256");
    assert_eq!(log[0]["code_challenge"], secrets.code_challenge);
    assert_eq!(log[0]["state"], secrets.state);
    assert_eq!(log[0]["nonce"], secrets.nonce);
    assert_eq!(log[0]["scope"], "openid email profile");
    assert!(!log[0].contains_key("prompt"));
    assert_eq!(log[1]["prompt"], "login");
}

#[tokio::test]
async fn a_wrong_or_missing_nonce_is_refused() {
    let (fake, provider) = setup().await;
    for nonce in [
        NonceMode::Fixed("not-the-nonce".to_owned()),
        NonceMode::Missing,
    ] {
        let result = login(
            &fake,
            &provider,
            LoginSpec {
                nonce,
                ..LoginSpec::default()
            },
        )
        .await;
        assert_eq!(refused(result), Refusal::NonceMismatch);
    }
}

#[tokio::test]
async fn audience_and_authorized_party_are_enforced() {
    let (fake, provider) = setup().await;
    let wrong_aud = login(
        &fake,
        &provider,
        LoginSpec {
            aud: Some(vec!["someone-else".to_owned()]),
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(wrong_aud), Refusal::AudienceMismatch);

    let wrong_azp = login(
        &fake,
        &provider,
        LoginSpec {
            azp: Some("other-client".to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(wrong_azp), Refusal::AuthorizedPartyMismatch);

    let several_without_azp = login(
        &fake,
        &provider,
        LoginSpec {
            aud: Some(vec![CLIENT_ID.to_owned(), "account".to_owned()]),
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(
        refused(several_without_azp),
        Refusal::AuthorizedPartyMismatch
    );

    let several_with_azp = login(
        &fake,
        &provider,
        LoginSpec {
            aud: Some(vec![CLIENT_ID.to_owned(), "account".to_owned()]),
            azp: Some(CLIENT_ID.to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;
    assert!(several_with_azp.is_ok(), "{several_with_azp:?}");
}

#[tokio::test]
async fn a_wrong_issuer_is_refused_byte_for_byte() {
    let (fake, provider) = setup().await;
    for iss in [
        "https://evil.example/realms/staff".to_owned(),
        // A trailing slash is a different string.
        format!("{}/", fake.issuer()),
    ] {
        let result = login(
            &fake,
            &provider,
            LoginSpec {
                iss: Some(iss),
                ..LoginSpec::default()
            },
        )
        .await;
        assert_eq!(refused(result), Refusal::IssuerMismatch);
    }
}

#[tokio::test]
async fn expired_and_future_dated_tokens_are_refused() {
    let (fake, provider) = setup().await;
    let expired = login(
        &fake,
        &provider,
        LoginSpec {
            exp_in_secs: -3600,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(expired), Refusal::Expired);

    let future = login(
        &fake,
        &provider,
        LoginSpec {
            iat_offset_secs: 3600,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(future), Refusal::IssuedInFuture);
}

#[tokio::test]
async fn an_unknown_kid_earns_exactly_one_refetch_and_is_then_refused() {
    let (fake, provider) = setup().await;
    // Prime the cache with one good sign-in.
    login(&fake, &provider, LoginSpec::default())
        .await
        .expect("ok");
    let before = fake.jwks_fetches();

    let result = login(
        &fake,
        &provider,
        LoginSpec {
            key: SigningKey::Rsa3,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(result), Refusal::UnknownKeyId);
    assert_eq!(fake.jwks_fetches(), before + 1, "one refetch, not a loop");
}

#[tokio::test]
async fn a_rotated_key_is_picked_up_by_the_single_refetch() {
    let (fake, provider) = setup().await;
    login(&fake, &provider, LoginSpec::default())
        .await
        .expect("ok");
    fake.publish_key(SigningKey::Rsa2);
    let claims = login(
        &fake,
        &provider,
        LoginSpec {
            key: SigningKey::Rsa2,
            ..LoginSpec::default()
        },
    )
    .await;
    assert!(claims.is_ok(), "{claims:?}");
}

#[tokio::test]
async fn refetches_are_rate_limited_by_the_minimum_interval() {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(&fake, &CLIENT_SECRET, 3600)).expect("provider");
    login(&fake, &provider, LoginSpec::default())
        .await
        .expect("ok");
    let before = fake.jwks_fetches();
    for _ in 0..3 {
        let result = login(
            &fake,
            &provider,
            LoginSpec {
                key: SigningKey::Rsa3,
                ..LoginSpec::default()
            },
        )
        .await;
        assert_eq!(refused(result), Refusal::UnknownKeyId);
    }
    assert_eq!(
        fake.jwks_fetches(),
        before,
        "unknown kids must not become a request amplifier against the IdP"
    );
}

#[tokio::test]
async fn a_known_kid_with_a_signature_from_another_key_is_refused() {
    let (fake, provider) = setup().await;
    let result = login(
        &fake,
        &provider,
        LoginSpec {
            key: SigningKey::Rsa3,
            kid: KidMode::Override(SigningKey::Rsa1.kid().to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(result), Refusal::SignatureInvalid);
}

#[tokio::test]
async fn unsigned_and_symmetric_tokens_and_a_missing_kid_are_refused() {
    let (fake, provider) = setup().await;
    let none = login(
        &fake,
        &provider,
        LoginSpec {
            alg: SigAlg::None,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(none), Refusal::AlgorithmNotAllowed);

    let hs256 = login(
        &fake,
        &provider,
        LoginSpec {
            alg: SigAlg::Hs256,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(hs256), Refusal::AlgorithmNotAllowed);

    let no_kid = login(
        &fake,
        &provider,
        LoginSpec {
            kid: KidMode::Missing,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(no_kid), Refusal::MissingKeyId);
}

#[tokio::test]
async fn es256_and_ps256_signed_tokens_are_accepted() {
    let (fake, provider) = setup().await;
    let es256 = login(
        &fake,
        &provider,
        LoginSpec {
            alg: SigAlg::Es256,
            key: SigningKey::Ec1,
            ..LoginSpec::default()
        },
    )
    .await;
    assert!(es256.is_ok(), "{es256:?}");
    // A key whose JWK states `alg: RS256` cannot be used for PS256 ...
    let mismatched = login(
        &fake,
        &provider,
        LoginSpec {
            alg: SigAlg::Ps256,
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(mismatched), Refusal::KeyUnsuitable);
    // ... one that states no `alg` can.
    fake.publish_key(SigningKey::Rsa2);
    let ps256 = login(
        &fake,
        &provider,
        LoginSpec {
            alg: SigAlg::Ps256,
            key: SigningKey::Rsa2,
            ..LoginSpec::default()
        },
    )
    .await;
    assert!(ps256.is_ok(), "{ps256:?}");
}

#[tokio::test]
async fn an_ec_key_cannot_validate_an_rsa_algorithm_token() {
    let (fake, provider) = setup().await;
    // RS256 header, but the kid names the EC key.
    let result = login(
        &fake,
        &provider,
        LoginSpec {
            kid: KidMode::Override(SigningKey::Ec1.kid().to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;
    assert_eq!(refused(result), Refusal::KeyUnsuitable);
}

#[tokio::test]
async fn a_discovery_issuer_that_differs_from_the_configured_one_is_not_used() {
    let (fake, provider) = setup().await;
    // Another host, and the same issuer in the spellings a normaliser would
    // accept: a trailing slash and a different case. Byte for byte, or refused.
    for claimed in [
        "https://somebody-else.example/realms/staff".to_owned(),
        format!("{}/", fake.issuer()),
        fake.issuer().to_uppercase(),
    ] {
        fake.set_discovery_issuer(&claimed);
        let secrets = AuthnSecrets::generate();
        let error = provider
            .authorization_url(&secrets, false)
            .await
            .expect_err("a mismatched discovery document must be refused");
        assert!(
            matches!(error, SsoError::Unavailable(_)),
            "{claimed}: {error:?}"
        );
    }
    assert_eq!(
        fake.jwks_fetches(),
        0,
        "no key is fetched from an unverified document"
    );
}

#[tokio::test]
async fn a_wrong_client_secret_is_an_operator_problem_not_a_user_refusal() {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(
        &fake,
        &sms_core::password::generate_password(24),
        0,
    ))
    .expect("provider");
    fake.queue_login(LoginSpec::default());
    let secrets = AuthnSecrets::generate();
    let (code, _) = authorize(&provider, &secrets, false).await;
    let error = provider
        .exchange_code(&code, &secrets.code_verifier)
        .await
        .expect_err("invalid_client");
    assert!(matches!(error, SsoError::Unavailable(_)), "{error:?}");
}

#[tokio::test]
async fn a_replayed_code_and_a_wrong_pkce_verifier_are_refused_upstream() {
    let (fake, provider) = setup().await;
    fake.queue_login(LoginSpec::default());
    let secrets = AuthnSecrets::generate();
    let (code, _) = authorize(&provider, &secrets, false).await;

    // A wrong verifier burns the code (single use at the IdP too).
    let wrong = provider
        .exchange_code(&code, "not-the-verifier-not-the-verifier-not-the-verifier")
        .await
        .expect_err("pkce mismatch");
    assert!(
        matches!(wrong, SsoError::Refused(Refusal::UpstreamDenied)),
        "{wrong:?}"
    );

    let replay = provider
        .exchange_code(&code, &secrets.code_verifier)
        .await
        .expect_err("a spent code");
    assert!(
        matches!(replay, SsoError::Refused(Refusal::UpstreamDenied)),
        "{replay:?}"
    );
}

// --- livemode, stale keys, and no queueing behind a dead identity provider ----

/// Under `livemode` every endpoint the discovery document names must be `https`,
/// not only the issuer: the fake's are plain `http` on loopback, which a developer's
/// Keycloak may be and a production gateway's code exchange must never be sent to.
#[tokio::test]
async fn under_livemode_a_discovery_naming_cleartext_endpoints_is_refused() {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let mut config = config_for(&fake, &CLIENT_SECRET, 0);
    config.livemode = true;
    let provider = SsoProvider::new(config).expect("provider");
    let error = provider
        .authorization_url(&AuthnSecrets::generate(), false)
        .await
        .expect_err("cleartext endpoints under livemode");
    assert!(matches!(error, SsoError::Unavailable(_)), "{error:?}");
    // The same document is fine outside livemode (every other test in this file).
    let (_fake, plain) = setup().await;
    plain
        .authorization_url(&AuthnSecrets::generate(), false)
        .await
        .expect("a sandbox may use loopback http");
}

/// A raw token from the fake, to verify more than once.
async fn raw_token(fake: &FakeOidc, provider: &SsoProvider, spec: LoginSpec) -> String {
    fake.queue_login(spec);
    let secrets = AuthnSecrets::generate();
    let (code, _) = authorize(provider, &secrets, false).await;
    provider
        .exchange_code(&code, &secrets.code_verifier)
        .await
        .expect("a token")
}

/// **A stale key beats none.** The keys are older than their lifetime and the
/// provider is gone: the refetch fails and the key held from before is used. A
/// provider that never had the key has nothing to fall back to and says so.
#[tokio::test]
async fn a_key_past_its_lifetime_is_used_when_the_refetch_fails() {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let config = config_for(&fake, &CLIENT_SECRET, 0);
    let provider = SsoProvider::with_cache_lifetimes(
        config.clone(),
        std::time::Duration::from_secs(3600),
        std::time::Duration::from_millis(1),
    )
    .expect("provider");
    let nonce: &'static str = fresh_nonce().leak();
    let raw = raw_token(
        &fake,
        &provider,
        LoginSpec {
            nonce: NonceMode::Fixed(nonce.to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;
    provider.verify_id_token(&raw, nonce).await.expect("fresh");

    let never_fetched = SsoProvider::new(config).expect("provider");
    drop(fake);
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    provider
        .verify_id_token(&raw, nonce)
        .await
        .expect("the key held from before is used while the provider is down");
    let result = never_fetched.verify_id_token(&raw, nonce).await;
    assert!(
        matches!(result, Err(SsoError::Unavailable(_))),
        "nothing held, nothing to fall back to: {result:?}"
    );
}

/// An identity provider that answers discovery and the first JWKS request and then
/// hangs on every later JWKS request. Returns its issuer.
async fn provider_that_hangs_after_the_first_keys(keys: String) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let issuer = format!("http://{}", listener.local_addr().expect("addr"));
    let served = issuer.clone();
    let first = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (issuer, keys, first) = (served.clone(), keys.clone(), first.clone());
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                let n = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let body = if request.contains("openid-configuration") {
                    serde_json::json!({
                        "issuer": issuer,
                        "authorization_endpoint": format!("{issuer}/authorize"),
                        "token_endpoint": format!("{issuer}/token"),
                        "jwks_uri": format!("{issuer}/jwks.json"),
                    })
                    .to_string()
                } else if first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    keys
                } else {
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                    return;
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    issuer
}

/// **A dead identity provider does not queue every sign-in behind its timeout.**
/// With the keys stale and a refetch already hanging, the other callers use the key
/// they hold and return at once; only the one making the fetch waits.
#[tokio::test]
async fn callers_do_not_queue_behind_a_hanging_refetch() {
    let fake = FakeOidc::start(CLIENT_ID, &CLIENT_SECRET).await;
    let keys = reqwest::get(format!("{}/jwks.json", fake.issuer()))
        .await
        .expect("keys")
        .text()
        .await
        .expect("body");
    let hanging = provider_that_hangs_after_the_first_keys(keys).await;

    // The token is minted by the fake and claims the hanging provider's issuer.
    let minting = SsoProvider::new(config_for(&fake, &CLIENT_SECRET, 0)).expect("provider");
    let nonce: &'static str = fresh_nonce().leak();
    let raw = raw_token(
        &fake,
        &minting,
        LoginSpec {
            iss: Some(hanging.clone()),
            nonce: NonceMode::Fixed(nonce.to_owned()),
            ..LoginSpec::default()
        },
    )
    .await;

    let mut config = config_for(&fake, &CLIENT_SECRET, 0);
    config.issuer = hanging;
    let provider = SsoProvider::with_cache_lifetimes(
        config,
        std::time::Duration::from_secs(3600),
        std::time::Duration::from_millis(1),
    )
    .expect("provider");
    provider.verify_id_token(&raw, nonce).await.expect("first");
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..5 {
        let (provider, raw) = (provider.clone(), raw.clone());
        tasks.spawn(async move { provider.verify_id_token(&raw, nonce).await.is_ok() });
    }
    let mut finished = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    while let Ok(Some(joined)) = tokio::time::timeout_at(deadline, tasks.join_next()).await {
        assert!(joined.expect("task"), "a stale key verifies");
        finished += 1;
    }
    assert!(
        finished >= 4,
        "{finished} of 5 returned within 3 s: callers queued behind the hanging fetch"
    );
    tasks.abort_all();
}

/// A nonce drawn at run time, so no test carries a fixed value in a nonce's place.
fn fresh_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
