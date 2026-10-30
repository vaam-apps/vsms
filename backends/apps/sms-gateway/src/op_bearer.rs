#![doc = include_str!("op_bearer.md")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use authkestra_engine::TokenManager;
use authkestra_engine::token::Claims;
use authkestra_engine::token::jwk::Jwk;
use cratestack::CratestackContext;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use sms_api::schema::Cratestack;
use sms_api::{BearerRefusal, BearerVerifier, VerifiedBearer};

use crate::op::OpKeyring;

/// The slack, in seconds, `exp` and `nbf` are given for clock skew between this
/// process and the one that minted the token. The library's own default, written
/// out because a verifier whose tolerance is implicit has no tolerance anyone can
/// review. Access tokens live fifteen minutes (`sms_config::ACCESS_TOKEN_TTL_SECS`),
/// so a minute either way is the whole of what this buys.
pub(crate) const LEEWAY_SECS: u64 = 60;

/// The least time between two reloads of the key set forced by a `kid` nobody
/// here has heard of. An attacker can send unlimited unknown `kid`s; this makes
/// each one cost the same single database read per interval, however many there
/// are.
pub(crate) const MIN_FORCED_RELOAD: Duration = Duration::from_secs(5);

/// Where a forced reload gets the key set from. A trait so the rate limit and the
/// single flight are tested without a database; the production one is
/// [`DbKeys`].
#[async_trait::async_trait]
pub(crate) trait KeyReload: Send + Sync + 'static {
    /// The signing key and the public key set as they are now.
    async fn reload(&self) -> anyhow::Result<(Arc<TokenManager>, Vec<Jwk>)>;
}

/// Reads the `OauthSigningKey` rows: exactly what `spawn_key_refresh` reads.
pub(crate) struct DbKeys {
    pub(crate) db: Cratestack,
    pub(crate) sys: CratestackContext,
    pub(crate) issuer: String,
}

#[async_trait::async_trait]
impl KeyReload for DbKeys {
    async fn reload(&self) -> anyhow::Result<(Arc<TokenManager>, Vec<Jwk>)> {
        sms_auth::op::load_signing_keys(&self.db, &self.sys, &self.issuer).await
    }
}

/// Verifies the access tokens this process's own OP mints.
///
/// The keys are the OP's: the keyring `/jwks.json` publishes from, read in
/// process. There is no HTTP fetch of our own JWKS (a bearer door that needs the
/// network to answer itself fails exactly when the gateway is struggling, and
/// can be pointed elsewhere), no key cache of its own to go stale, and no second
/// opinion about which key is current.
pub(crate) struct OpBearerVerifier {
    keys: Arc<OpKeyring>,
    reload: Arc<dyn KeyReload>,
    /// When the last forced reload started; held across a reload so concurrent
    /// unknown `kid`s wait for the one in flight instead of starting their own.
    last_forced: tokio::sync::Mutex<Option<Instant>>,
    min_reload: Duration,
    validation: Validation,
}

impl OpBearerVerifier {
    /// A verifier for tokens issued by `issuer`, using `keys`.
    pub(crate) fn new(keys: Arc<OpKeyring>, reload: Arc<dyn KeyReload>, issuer: &str) -> Self {
        Self::with_reload_interval(keys, reload, issuer, MIN_FORCED_RELOAD)
    }

    pub(crate) fn with_reload_interval(
        keys: Arc<OpKeyring>,
        reload: Arc<dyn KeyReload>,
        issuer: &str,
        min_reload: Duration,
    ) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[issuer]);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        // `aud` is checked by hand in `verify`, against the token's own `sub`: the
        // library takes one fixed audience list, and what a machine token is
        // addressed to is its own client (authkestra stamps `aud = client_id`).
        // Left on with no list set it would refuse every token, which is how
        // switching it off is easy to mistake for a hole; it is a hole only if
        // the hand check below is missing, and a test fails when it is.
        validation.validate_aud = false;
        validation.required_spec_claims = ["exp", "iss", "sub", "aud"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        Self {
            keys,
            reload,
            last_forced: tokio::sync::Mutex::new(None),
            min_reload,
            validation,
        }
    }

    /// The key `kid` names, forcing one reload of the key set when it is unknown
    /// (a rotation another process ran, noticed before the periodic refresh does).
    async fn key_for(&self, kid: &str) -> Option<Arc<jsonwebtoken::DecodingKey>> {
        if let Some(key) = self.keys.decoding_key(kid) {
            return Some(key);
        }
        let mut last = self.last_forced.lock().await;
        // Whoever held the lock before us may have just loaded it.
        if let Some(key) = self.keys.decoding_key(kid) {
            return Some(key);
        }
        if last.is_some_and(|at| at.elapsed() < self.min_reload) {
            return None;
        }
        *last = Some(Instant::now());
        match self.reload.reload().await {
            Ok((tokens, jwks)) => self.keys.refresh(tokens, jwks),
            Err(error) => {
                tracing::warn!(%error, "reloading the OP signing keys for an unknown kid failed");
            }
        }
        self.keys.decoding_key(kid)
    }
}

fn refusal_of(error: &jsonwebtoken::errors::Error) -> BearerRefusal {
    match error.kind() {
        ErrorKind::ExpiredSignature => BearerRefusal::Expired,
        ErrorKind::ImmatureSignature => BearerRefusal::NotYetValid,
        ErrorKind::InvalidIssuer => BearerRefusal::Issuer,
        ErrorKind::InvalidAudience => BearerRefusal::Audience,
        ErrorKind::InvalidSignature => BearerRefusal::Signature,
        ErrorKind::MissingRequiredClaim(claim) => match claim.as_str() {
            "iss" => BearerRefusal::Issuer,
            "aud" => BearerRefusal::Audience,
            _ => BearerRefusal::Malformed,
        },
        _ => BearerRefusal::Malformed,
    }
}

#[async_trait::async_trait]
impl BearerVerifier for OpBearerVerifier {
    async fn verify(&self, token: &str) -> Result<VerifiedBearer, BearerRefusal> {
        let header = decode_header(token).map_err(|_| BearerRefusal::Malformed)?;
        if header.alg != Algorithm::RS256 {
            return Err(BearerRefusal::Malformed);
        }
        let kid = header.kid.ok_or(BearerRefusal::MissingKid)?;
        let key = self.key_for(&kid).await.ok_or(BearerRefusal::UnknownKid)?;
        let claims = decode::<Claims>(token, &key, &self.validation)
            .map_err(|error| refusal_of(&error))?
            .claims;

        // A person's token is not a machine's: only `client_credentials` tokens
        // have a door here, and those carry no identity.
        if claims.identity.is_some() {
            return Err(BearerRefusal::HumanToken);
        }
        // Addressed to its own client, and to nothing else that excludes it.
        if claims.sub.is_empty()
            || !claims
                .aud
                .as_ref()
                .is_some_and(|aud| aud.contains(&claims.sub))
        {
            return Err(BearerRefusal::Audience);
        }
        Ok(VerifiedBearer {
            client_id: claims.sub,
            scope: claims.scope,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use jsonwebtoken::{EncodingKey, Header, encode};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use serde_json::{Value, json};

    use super::*;
    use crate::op::{Keyring, PublishedKeys};

    const ISSUER: &str = "https://sms.example.com";
    const CLIENT: &str = "appc_demo_client";

    /// One RSA key a test signs with, and the `TokenManager` the OP would hold
    /// for it. Generated here so no private key is committed.
    struct Key {
        kid: String,
        pem: String,
        manager: Arc<TokenManager>,
    }

    fn key(kid: &str) -> Key {
        let pem = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
            .expect("generating a test key")
            .to_pkcs8_pem(LineEnding::LF)
            .expect("encoding it")
            .to_string();
        let manager = Arc::new(
            TokenManager::new_asymmetric(
                pem.as_bytes(),
                Some(ISSUER.to_owned()),
                Some(kid.to_owned()),
            )
            .expect("a manager for the key"),
        );
        Key {
            kid: kid.to_owned(),
            pem,
            manager,
        }
    }

    /// Two keys, generated once for the whole module: RSA generation is the slow
    /// part of every test here.
    fn keys() -> &'static (Key, Key) {
        static KEYS: std::sync::OnceLock<(Key, Key)> = std::sync::OnceLock::new();
        KEYS.get_or_init(|| (key("kid-a"), key("kid-b")))
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// The claims `authkestra` stamps on a `client_credentials` token, each
    /// overridable by `patch` (a `null` removes the claim).
    fn claims(patch: &Value) -> Value {
        let mut claims = json!({
            "iss": ISSUER,
            "sub": CLIENT,
            "aud": CLIENT,
            "exp": now() + 900,
            "iat": now(),
            "scope": "sms:send sms:read",
        });
        for (name, value) in patch.as_object().expect("a patch is an object") {
            if value.is_null() {
                claims.as_object_mut().unwrap().remove(name);
            } else {
                claims[name] = value.clone();
            }
        }
        claims
    }

    fn mint(signer: &Key, header_kid: Option<&str>, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = header_kid.map(str::to_owned);
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(signer.pem.as_bytes()).unwrap(),
        )
        .expect("signing the test token")
    }

    /// Counts the reloads and answers with `then`, optionally holding the first
    /// one open so a test can pile concurrent callers behind it.
    struct Counting {
        calls: AtomicUsize,
        then: Mutex<Vec<(Arc<TokenManager>, Vec<Jwk>)>>,
    }

    #[async_trait::async_trait]
    impl KeyReload for Counting {
        async fn reload(&self) -> anyhow::Result<(Arc<TokenManager>, Vec<Jwk>)> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            self.then
                .lock()
                .unwrap()
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("nothing to reload"))
        }
    }

    fn keyring(published: &[&Key]) -> Arc<OpKeyring> {
        Arc::new(Keyring::new(
            published[0].manager.clone(),
            PublishedKeys::new(
                published
                    .iter()
                    .filter_map(|key| key.manager.public_jwk())
                    .collect(),
            ),
        ))
    }

    fn verifier(
        published: &[&Key],
        reload: Arc<Counting>,
        min_reload: Duration,
    ) -> OpBearerVerifier {
        OpBearerVerifier::with_reload_interval(keyring(published), reload, ISSUER, min_reload)
    }

    fn quiet() -> Arc<Counting> {
        Arc::new(Counting {
            calls: AtomicUsize::new(0),
            then: Mutex::new(Vec::new()),
        })
    }

    fn plain(published: &[&Key]) -> OpBearerVerifier {
        verifier(published, quiet(), MIN_FORCED_RELOAD)
    }

    async fn refusal(verifier: &OpBearerVerifier, token: &str) -> BearerRefusal {
        verifier
            .verify(token)
            .await
            .expect_err("the token is refused")
    }

    #[tokio::test]
    async fn a_token_the_op_minted_verifies_and_names_its_client_and_scope() {
        let (a, _) = keys();
        let token = mint(a, Some(&a.kid), &claims(&json!({})));
        let verified = plain(&[a]).verify(&token).await.expect("verifies");
        assert_eq!(
            verified,
            VerifiedBearer {
                client_id: CLIENT.to_owned(),
                scope: Some("sms:send sms:read".to_owned()),
            }
        );
        // No scope claim: the client's own scopes apply, decided downstream.
        let token = mint(a, Some(&a.kid), &claims(&json!({"scope": null})));
        assert_eq!(plain(&[a]).verify(&token).await.unwrap().scope, None);
    }

    /// The token as the OP's own `TokenManager` issues it, not as these tests
    /// hand-build one: the claim names and the `kid` header are the real ones.
    #[tokio::test]
    async fn a_token_issued_by_the_token_manager_verifies() {
        let (a, _) = keys();
        let token = a
            .manager
            .issue_client_token_with_extra(
                CLIENT,
                900,
                Some("sms:send".to_owned()),
                Some(CLIENT.to_owned()),
                std::collections::HashMap::new(),
            )
            .expect("issuing");
        let verified = plain(&[a]).verify(&token).await.expect("verifies");
        assert_eq!(verified.client_id, CLIENT);
        assert_eq!(verified.scope.as_deref(), Some("sms:send"));
    }

    #[tokio::test]
    async fn another_issuer_is_refused() {
        let (a, _) = keys();
        let token = mint(
            a,
            Some(&a.kid),
            &claims(&json!({"iss": "https://evil.example"})),
        );
        assert_eq!(refusal(&plain(&[a]), &token).await, BearerRefusal::Issuer);
        let token = mint(a, Some(&a.kid), &claims(&json!({"iss": null})));
        assert_eq!(refusal(&plain(&[a]), &token).await, BearerRefusal::Issuer);
    }

    #[tokio::test]
    async fn an_audience_that_leaves_out_the_subject_is_refused() {
        let (a, _) = keys();
        let v = plain(&[a]);
        for aud in [
            json!("vsms"),
            json!("another_client"),
            json!(["x", "y"]),
            json!([]),
        ] {
            let token = mint(a, Some(&a.kid), &claims(&json!({ "aud": aud })));
            assert_eq!(refusal(&v, &token).await, BearerRefusal::Audience, "{aud}");
        }
        let token = mint(a, Some(&a.kid), &claims(&json!({"aud": null})));
        assert_eq!(refusal(&v, &token).await, BearerRefusal::Audience, "no aud");
        // A list that contains `sub` is addressed to it.
        let token = mint(a, Some(&a.kid), &claims(&json!({"aud": ["other", CLIENT]})));
        assert!(v.verify(&token).await.is_ok());
    }

    #[tokio::test]
    async fn expiry_has_a_minute_of_leeway_and_no_more() {
        let (a, _) = keys();
        let v = plain(&[a]);
        let token = mint(a, Some(&a.kid), &claims(&json!({"exp": now() - 30})));
        assert!(
            v.verify(&token).await.is_ok(),
            "30 seconds late is inside the leeway"
        );
        let token = mint(a, Some(&a.kid), &claims(&json!({"exp": now() - 3600})));
        assert_eq!(refusal(&v, &token).await, BearerRefusal::Expired);
        let token = mint(a, Some(&a.kid), &claims(&json!({"exp": null})));
        assert_eq!(
            refusal(&v, &token).await,
            BearerRefusal::Malformed,
            "exp is required"
        );
    }

    #[tokio::test]
    async fn a_token_not_valid_yet_is_refused() {
        let (a, _) = keys();
        let v = plain(&[a]);
        let token = mint(a, Some(&a.kid), &claims(&json!({"nbf": now() + 3600})));
        assert_eq!(refusal(&v, &token).await, BearerRefusal::NotYetValid);
        let token = mint(a, Some(&a.kid), &claims(&json!({"nbf": now() + 30})));
        assert!(v.verify(&token).await.is_ok(), "inside the leeway");
    }

    /// With two keys published during a rotation, a token with no `kid` must not
    /// be tried against whichever is listed first.
    #[tokio::test]
    async fn a_token_without_a_kid_is_refused_even_when_a_key_would_verify_it() {
        let (a, b) = keys();
        let token = mint(a, None, &claims(&json!({})));
        assert_eq!(
            refusal(&plain(&[a, b]), &token).await,
            BearerRefusal::MissingKid
        );
    }

    #[tokio::test]
    async fn a_signature_by_another_key_is_refused() {
        let (a, b) = keys();
        // Signed with b, claiming a's `kid`.
        let token = mint(b, Some(&a.kid), &claims(&json!({})));
        assert_eq!(
            refusal(&plain(&[a, b]), &token).await,
            BearerRefusal::Signature
        );
        // A tampered payload under a good signature.
        let good = mint(a, Some(&a.kid), &claims(&json!({})));
        let mut parts: Vec<&str> = good.split('.').collect();
        let forged = mint(a, Some(&a.kid), &claims(&json!({"sub": "appc_victim"})));
        let forged_payload = forged.split('.').nth(1).unwrap();
        parts[1] = forged_payload;
        assert_eq!(
            refusal(&plain(&[a]), &parts.join(".")).await,
            BearerRefusal::Signature
        );
    }

    #[tokio::test]
    async fn anything_but_a_well_formed_rs256_jwt_is_malformed() {
        let (a, _) = keys();
        let v = plain(&[a]);
        for token in [
            "",
            "abc",
            "a.b",
            "a.b.c",
            "....",
            "eyJhbGciOiJub25lIn0.e30.",
        ] {
            assert_eq!(
                refusal(&v, token).await,
                BearerRefusal::Malformed,
                "{token:?}"
            );
        }
    }

    /// The algorithm-confusion attack: the public key is, well, public, so an
    /// attacker signs an HS256 token using its bytes as the HMAC secret and
    /// hopes the verifier reads the algorithm from the token and then "verifies"
    /// with the key it holds, as a secret. Every encoding of the key an attacker
    /// can reach is tried (`/jwks.json` publishes the modulus and exponent; PEM
    /// and DER are one conversion away), under the genuine `kid`, with
    /// otherwise perfect claims.
    #[tokio::test]
    async fn an_hs256_token_keyed_with_the_public_key_is_refused() {
        use base64::Engine as _;
        use rsa::pkcs1::EncodeRsaPublicKey;
        use rsa::pkcs8::{DecodePrivateKey, EncodePublicKey};
        use rsa::traits::PublicKeyParts;

        let (a, _) = keys();
        let public = RsaPrivateKey::from_pkcs8_pem(&a.pem)
            .expect("the test key parses")
            .to_public_key();
        let published = a.manager.public_jwk().expect("the key publishes a JWK");
        let modulus = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(published.n.expect("an RSA JWK has a modulus"))
            .expect("a base64url modulus");
        assert_eq!(modulus, public.n().to_bytes_be(), "the JWK is this key");
        let secrets: [(&str, Vec<u8>); 5] = [
            (
                "SPKI PEM",
                public
                    .to_public_key_pem(LineEnding::LF)
                    .unwrap()
                    .into_bytes(),
            ),
            (
                "SPKI DER",
                public.to_public_key_der().unwrap().as_bytes().to_vec(),
            ),
            (
                "PKCS#1 PEM",
                public
                    .to_pkcs1_pem(LineEnding::LF)
                    .unwrap()
                    .into_bytes(),
            ),
            (
                "PKCS#1 DER",
                public.to_pkcs1_der().unwrap().as_bytes().to_vec(),
            ),
            ("the modulus", modulus),
        ];
        let v = plain(&[a]);
        for (what, secret) in secrets {
            for algorithm in [Algorithm::HS256, Algorithm::HS384, Algorithm::HS512] {
                let mut header = Header::new(algorithm);
                header.kid = Some(a.kid.clone());
                let forged = encode(
                    &header,
                    &claims(&json!({})),
                    &EncodingKey::from_secret(&secret),
                )
                .expect("signing the forgery");
                assert_eq!(
                    refusal(&v, &forged).await,
                    BearerRefusal::Malformed,
                    "{algorithm:?} keyed with {what}"
                );
            }
        }
        // The same claims, signed properly, are accepted: the refusals above are
        // about the algorithm and not about the claims.
        let genuine = mint(a, Some(&a.kid), &claims(&json!({})));
        assert!(v.verify(&genuine).await.is_ok());
    }

    #[tokio::test]
    async fn a_persons_token_is_not_a_machines() {
        let (a, _) = keys();
        let identity = json!({
            "provider_id": "console", "external_id": "usr1",
            "email": null, "username": null, "attributes": {}
        });
        let token = mint(a, Some(&a.kid), &claims(&json!({ "identity": identity })));
        assert_eq!(
            refusal(&plain(&[a]), &token).await,
            BearerRefusal::HumanToken
        );
    }

    #[tokio::test]
    async fn an_unknown_kid_reloads_the_key_set_once_and_then_verifies() {
        let (a, b) = keys();
        let reload = quiet();
        reload.then.lock().unwrap().push((
            b.manager.clone(),
            vec![
                a.manager.public_jwk().unwrap(),
                b.manager.public_jwk().unwrap(),
            ],
        ));
        // The process knows only `a`; another replica rotated to `b`.
        let v = verifier(&[a], reload.clone(), Duration::from_secs(5));
        let token = mint(b, Some(&b.kid), &claims(&json!({})));
        assert!(v.verify(&token).await.is_ok());
        assert_eq!(reload.calls.load(Ordering::SeqCst), 1);
        // Now known: no second reload.
        assert!(v.verify(&token).await.is_ok());
        assert_eq!(reload.calls.load(Ordering::SeqCst), 1);
    }

    /// A flood of invented `kid`s costs one read per interval, and a `kid` that
    /// the reload does not produce stays unknown.
    #[tokio::test]
    async fn forced_reloads_are_rate_limited_and_single_flight() {
        let (a, b) = keys();
        let reload = quiet(); // reloads fail: nothing to publish
        let v = Arc::new(verifier(&[a], reload.clone(), Duration::from_secs(60)));
        let token = mint(b, Some(&b.kid), &claims(&json!({})));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let (v, token) = (v.clone(), token.clone());
            handles.push(tokio::spawn(async move { v.verify(&token).await }));
        }
        for handle in handles {
            assert_eq!(
                handle.await.unwrap().unwrap_err(),
                BearerRefusal::UnknownKid
            );
        }
        assert_eq!(
            reload.calls.load(Ordering::SeqCst),
            1,
            "sixteen concurrent unknown kids are one read"
        );
        // Still inside the interval: another invented kid reads nothing.
        let other = mint(b, Some("kid-invented"), &claims(&json!({})));
        assert_eq!(refusal(&v, &other).await, BearerRefusal::UnknownKid);
        assert_eq!(reload.calls.load(Ordering::SeqCst), 1);
    }

    /// The callers queued behind the one reload do not each give up because the
    /// interval has not elapsed: the key the first one loaded is theirs too.
    #[tokio::test]
    async fn callers_queued_behind_a_reload_use_the_key_it_loaded() {
        let (a, b) = keys();
        let reload = quiet();
        reload.then.lock().unwrap().push((
            b.manager.clone(),
            vec![
                a.manager.public_jwk().unwrap(),
                b.manager.public_jwk().unwrap(),
            ],
        ));
        let v = Arc::new(verifier(&[a], reload.clone(), Duration::from_secs(60)));
        let token = mint(b, Some(&b.kid), &claims(&json!({})));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let (v, token) = (v.clone(), token.clone());
            handles.push(tokio::spawn(async move { v.verify(&token).await }));
        }
        for handle in handles {
            assert!(
                handle.await.unwrap().is_ok(),
                "every queued caller verifies"
            );
        }
        assert_eq!(reload.calls.load(Ordering::SeqCst), 1, "behind one read");
    }

    #[tokio::test]
    async fn the_interval_elapsing_allows_the_next_forced_reload() {
        let (a, b) = keys();
        let reload = quiet();
        let v = verifier(&[a], reload.clone(), Duration::from_millis(20));
        let token = mint(b, Some(&b.kid), &claims(&json!({})));
        assert_eq!(refusal(&v, &token).await, BearerRefusal::UnknownKid);
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(refusal(&v, &token).await, BearerRefusal::UnknownKid);
        assert_eq!(reload.calls.load(Ordering::SeqCst), 2);
    }
}
