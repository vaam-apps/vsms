//! The SSO broker's upstream half, against a scripted fake `IdP`
//! (`sms-fake-oidc`) over real HTTP: discovery, the JWKS, the authorization
//! URL, the `client_secret_basic` code exchange, and `id_token`
//! verification with tokens that are *actually* malformed - not helpers
//! asserted in isolation.
//!
//! No database: nothing here touches Postgres, so unlike the live suites
//! these run in the ordinary `cargo test --workspace`.

use sms_auth::sso::{
    AuthnSecrets, IdClaims, RawSsoConfig, Refusal, SsoConfig, SsoError, SsoProvider,
};
use sms_fake_oidc::{FakeOidc, KidMode, LoginSpec, NonceMode, SigAlg, SigningKey};

const CLIENT_ID: &str = "vsms-test";
const CLIENT_SECRET: &str = "test-secret+with/odd=chars";

fn config_for(fake: &FakeOidc, secret: &str, min_refresh_secs: u64) -> SsoConfig {
    SsoConfig::from_raw(RawSsoConfig {
        issuer: Some(fake.issuer().to_owned()),
        client_id: Some(CLIENT_ID.to_owned()),
        client_secret: Some(secret.to_owned()),
        redirect_uri: Some("http://127.0.0.1:1/login/sso/callback".to_owned()),
        required_group: Some("vsms-staff".to_owned()),
        accepted_amr: vec!["otp".to_owned()],
        jwks_min_refresh_secs: Some(min_refresh_secs),
        ..RawSsoConfig::default()
    })
    .expect("a valid config")
    .expect("SSO configured")
}

async fn setup() -> (FakeOidc, SsoProvider) {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(&fake, CLIENT_SECRET, 0)).expect("provider");
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
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(&fake, CLIENT_SECRET, 3600)).expect("provider");
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
    fake.set_discovery_issuer("https://somebody-else.example/realms/staff");
    let secrets = AuthnSecrets::generate();
    let error = provider
        .authorization_url(&secrets, false)
        .await
        .expect_err("a mismatched discovery document must be refused");
    assert!(matches!(error, SsoError::Unavailable(_)), "{error:?}");
    assert_eq!(
        fake.jwks_fetches(),
        0,
        "no key is fetched from an unverified document"
    );
}

#[tokio::test]
async fn a_wrong_client_secret_is_an_operator_problem_not_a_user_refusal() {
    let fake = FakeOidc::start(CLIENT_ID, CLIENT_SECRET).await;
    let provider = SsoProvider::new(config_for(&fake, "wrong-secret", 0)).expect("provider");
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
