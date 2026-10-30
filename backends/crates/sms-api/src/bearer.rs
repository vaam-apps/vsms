#![doc = include_str!("bearer.md")]

use std::sync::Arc;

use cratestack::axum::extract::{Request, State};
use cratestack::axum::http::HeaderMap;
use cratestack::axum::http::header::AUTHORIZATION;
use cratestack::axum::middleware::Next;
use cratestack::axum::response::Response;
use cratestack::ratelimit::VerifiedPrincipal;
use cratestack::{CratestackError, UNAUTHENTICATED, VerifiedSigner};

use crate::rbac::error_response;
use crate::signers::{SignerDirectory, Standing};

/// Why a bearer token was refused, in the vocabulary of the check that failed.
///
/// **Never sent to the caller.** The door answers every one of these with the
/// same coarse 401 a forged signature gets, and writes the variant at `debug`
/// level, so a token that is merely expired cannot be told from one that was
/// never ours. The variants exist so a test can name the check that fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BearerRefusal {
    /// Not three base64url segments, an unreadable header or claims, or an
    /// algorithm other than the one this deployment signs with.
    #[error("the token is not a well-formed JWT")]
    Malformed,
    /// The header carries no `kid`. A key set with two keys during a rotation
    /// would otherwise be tried in whatever order it lists them.
    #[error("the token names no signing key")]
    MissingKid,
    /// The `kid` is in no key this deployment has signed with, even after one
    /// reload of the key set.
    #[error("the token names a key this deployment does not know")]
    UnknownKid,
    /// The signature does not verify under the key the `kid` names.
    #[error("the token's signature does not verify")]
    Signature,
    /// `iss` is not this deployment's `op.issuer`.
    #[error("the token was issued by another provider")]
    Issuer,
    /// `aud` does not contain `sub`: the token was minted for another resource.
    #[error("the token is not addressed to its own client")]
    Audience,
    /// `exp` is past, beyond the clock-skew leeway.
    #[error("the token has expired")]
    Expired,
    /// `nbf` is in the future, beyond the clock-skew leeway.
    #[error("the token is not valid yet")]
    NotYetValid,
    /// The token belongs to a person (it carries an `identity`) and this
    /// deployment serves no person realm (`human_login` is not served): only a
    /// machine's token has a door here then. Where `human_login` **is** served
    /// a person's token is accepted ([`VerifiedBearer::Person`]) and this
    /// refusal never fires.
    #[error("the token belongs to a person, and this deployment accepts only a machine's")]
    HumanToken,
    /// The token belongs to a machine client (no `identity`) and this deployment
    /// serves only people (`human_login` without `machine_tokens`). The mirror
    /// of [`BearerRefusal::HumanToken`].
    #[error("the token belongs to a machine client, and this deployment accepts only a person's")]
    MachineToken,
    /// The token is a person's **ID token**, not their access token. The OP
    /// stamps the two with the same issuer, subject, audience, identity and key;
    /// an ID token differs only by carrying no `scope` (and, when the sign-in
    /// asked for one, a `nonce`). It is the token a console reads to learn who
    /// signed in, handed to the browser and logged, never one to call the API
    /// with.
    #[error("the token is an ID token, not an access token")]
    IdToken,
    /// The token carries a `cnf` (confirmation) claim: it is bound to a key the
    /// holder must prove, and this door checks no proof. Accepted as a plain
    /// bearer it would lose the constraint it was issued with, so it is
    /// refused.
    #[error("the token is sender-constrained, and this door checks no proof of the key")]
    SenderConstrained,
}

/// What a verified token says, and nothing it does not.
///
/// Who the token names and, for a machine, the scopes it asked for: **claims
/// that narrow, never grant**. The caller's standing is read from the database
/// by [`SignerDirectory::bearer_standing`]; this enum is only how the token
/// names the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedBearer {
    /// A `client_credentials` token: no `identity`, `sub` is an
    /// `OauthClient.clientId`.
    Machine {
        /// The token's `sub`, an `OauthClient.clientId`.
        client_id: String,
        /// The token's space-delimited `scope` claim, when it has one. The
        /// effective scope is its intersection with the client's own.
        scope: Option<String>,
    },
    /// An `authorization_code` access token minted by `human_login`: an
    /// `identity` whose `external_id` is the token's `sub`, addressed to the
    /// console client alone. Its `scope` (the OIDC `openid profile`) is
    /// deliberately not carried: it names what the console may read about the
    /// person, and grants the API nothing; the person's role does.
    Person {
        /// The token's `sub`, a `User.id`.
        user_id: String,
        /// The token's one audience, the console's `OauthClient.clientId`.
        client_id: String,
    },
}

impl VerifiedBearer {
    /// Which realm the token belongs to, for a log line that must name neither
    /// the person nor the client.
    #[must_use]
    pub fn realm(&self) -> &'static str {
        match self {
            VerifiedBearer::Machine { .. } => "machine",
            VerifiedBearer::Person { .. } => "person",
        }
    }
}

/// Checks a bearer token's signature and claims.
///
/// An implementation owns the keys and the claim rules (`sms-gateway`'s is
/// `OpBearerVerifier`, over the key set the provider itself signs with); this
/// crate owns what a verified token is allowed to become. The seam is a trait
/// so the door's behaviour, which is the part with consequences, is tested with
/// a verifier that needs no RSA key.
#[async_trait::async_trait]
pub trait BearerVerifier: Send + Sync + 'static {
    /// The verified claims of `token`, or the check that refused it.
    async fn verify(&self, token: &str) -> Result<VerifiedBearer, BearerRefusal>;
}

/// The ways in a deployment has opened, from `auth.methods`.
///
/// A struct and not two bools: the router takes it beside six other positional
/// arguments, and `router(.., true, false)` does not say which door is shut.
/// Built by [`ApiDoors::new`], which refuses the shape with no door at all, the
/// one that would answer every request with a 401.
#[derive(Clone)]
pub struct ApiDoors {
    keypair: bool,
    bearer: Option<Arc<dyn BearerVerifier>>,
}

/// Neither door open: `auth.methods` permits no way to say who a caller is (a
/// person's token is a way in only beside the keypair or `machine_tokens`: it
/// is not one on its own, `ConfigError::NoWayIn`). The configuration refuses it
/// first; this is the same refusal at the place that would otherwise build a
/// gateway nobody can call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no way in: neither the signed envelope nor a machine token is accepted")]
pub struct NoDoor;

impl ApiDoors {
    /// `keypair`: `auth.methods` permits it. `bearer`: `Some` exactly when
    /// `machine_tokens` **or** `human_login` is served (permitted and its block
    /// written): the one verifier holds the realms, and which of them it
    /// accepts is its own business, not the doors'.
    ///
    /// # Errors
    ///
    /// [`NoDoor`] when `keypair` is off and there is no verifier.
    pub fn new(keypair: bool, bearer: Option<Arc<dyn BearerVerifier>>) -> Result<Self, NoDoor> {
        if !keypair && bearer.is_none() {
            return Err(NoDoor);
        }
        Ok(Self { keypair, bearer })
    }

    /// The signed envelope alone: what the API was from VS-3 to VSA-3a, and what
    /// a deployment that serves no tokens still is.
    #[must_use]
    pub fn keypair_only() -> Self {
        Self {
            keypair: true,
            bearer: None,
        }
    }

    /// Whether a signed `COSE_Sign1` request is accepted.
    #[must_use]
    pub fn keypair(&self) -> bool {
        self.keypair
    }

    /// Whether a bearer token is accepted.
    #[must_use]
    pub fn bearer(&self) -> bool {
        self.bearer.is_some()
    }
}

impl std::fmt::Debug for ApiDoors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiDoors")
            .field("keypair", &self.keypair)
            .field("bearer", &self.bearer())
            .finish()
    }
}

/// The caller a bearer token resolved to, recorded in the request's extensions
/// by [`enforce_bearer_door`] for [`crate::auth::GatewayAuth`] to read.
///
/// The bearer door's counterpart of `VerifiedSigner`: a fact an in-process layer
/// recorded, never something a request can carry (extensions do not cross the
/// wire). It holds the already-resolved [`Standing`], so the generated handler
/// and every Layer 2 guard that authenticates the same request again read the
/// same answer without asking the database twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerCaller(pub(crate) Standing);

impl BearerCaller {
    /// What the token stands as.
    #[must_use]
    pub fn standing(&self) -> &Standing {
        &self.0
    }
}

/// State for [`enforce_bearer_door`].
#[derive(Clone)]
pub struct BearerDoorState {
    pub(crate) doors: ApiDoors,
    pub(crate) directory: SignerDirectory,
}

/// The token in `Authorization: Bearer <token>`, when the request carries
/// exactly one such header.
///
/// # Errors
///
/// [`BearerRefusal::Malformed`] for no header, more than one, another scheme or
/// an empty token.
pub fn bearer_token(headers: &HeaderMap) -> Result<&str, BearerRefusal> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(BearerRefusal::Malformed);
    };
    let text = value.to_str().map_err(|_| BearerRefusal::Malformed)?;
    let (scheme, token) = text.split_once(' ').ok_or(BearerRefusal::Malformed)?;
    let token = token.trim();
    if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
        Ok(token)
    } else {
        Err(BearerRefusal::Malformed)
    }
}

fn unauthenticated() -> Response {
    error_response(&CratestackError::Unauthorized(UNAUTHENTICATED.to_owned()))
}

/// The second door, between the envelope layer and the per-principal limiter.
///
/// What it decides, in order:
///
/// 1. **A signed request that also carries `Authorization` is a 400.** One
///    caller, one credential: a client that sends both has not decided who it
///    is, and neither is trusted to settle it. Checked before anything else so
///    the answer does not depend on which door is open.
/// 2. **A signed request otherwise passes**: the envelope already verified it
///    and recorded the signer.
/// 3. **An unsigned request needs a verified bearer**, or it is the coarse 401
///    the envelope gives every unauthenticated caller. The token is checked
///    ([`BearerVerifier`]), then resolved to a standing
///    ([`SignerDirectory::bearer_standing`]: for a machine, the `OauthClient`
///    is active, its `AppClient` live and the same client; for a person, the
///    console client is the unlinked public one and the `User` is active with
///    a role that may act), and only then does the request become a principal:
///    `VerifiedPrincipal("client:<clientId>")` for a machine and
///    `VerifiedPrincipal("user:<User.id>")` for a person, the very names the
///    signed door gives the same caller, so both doors draw from **one**
///    rate-limit bucket and **one** idempotency namespace.
///
/// A refusal is one 401 whatever failed; the [`BearerRefusal`] is written at
/// `debug`. A database failure while resolving the standing is a 5xx, never a
/// refusal. **The one answer that is not a 401** is a person whose role is
/// `owner` or `admin`: a verified token and a live account, refused with a 403
/// ([`crate::auth::DEVICE_REQUIRED`]) because those roles act only from a paired
/// console key. That is a refusal of what the token may do, after it has proved
/// who holds it, so it does not hide behind the coarse 401.
pub async fn enforce_bearer_door(
    State(state): State<BearerDoorState>,
    mut request: Request,
    next: Next,
) -> Response {
    let signed = request.extensions().get::<VerifiedSigner>().is_some();
    if signed {
        if request.headers().contains_key(AUTHORIZATION) {
            return error_response(&CratestackError::BadRequest(
                "a signed request cannot also carry an Authorization header: send one credential"
                    .to_owned(),
            ));
        }
        return next.run(request).await;
    }

    let Some(checker) = state.doors.bearer.as_ref() else {
        return unauthenticated();
    };
    let claims = match authenticate_bearer(checker.as_ref(), request.headers()).await {
        Ok(claims) => claims,
        Err(refusal) => {
            tracing::debug!(%refusal, "a bearer token was refused");
            return unauthenticated();
        }
    };
    let standing = match state.directory.bearer_standing(&claims).await {
        Ok(standing) => standing,
        Err(CratestackError::Unauthorized(_)) => {
            tracing::debug!(
                realm = claims.realm(),
                "a verified token names no one who may act"
            );
            return unauthenticated();
        }
        Err(error) => return error_response(&error),
    };
    request
        .extensions_mut()
        .insert(VerifiedPrincipal(standing.principal()));
    request.extensions_mut().insert(BearerCaller(standing));
    next.run(request).await
}

async fn authenticate_bearer(
    verifier: &dyn BearerVerifier,
    headers: &HeaderMap,
) -> Result<VerifiedBearer, BearerRefusal> {
    verifier.verify(bearer_token(headers)?).await
}

#[cfg(test)]
mod tests {
    use cratestack::axum::Router;
    use cratestack::axum::body::Body;
    use cratestack::axum::http::{Request as HttpRequest, StatusCode};
    use cratestack::axum::middleware::{Next, from_fn, from_fn_with_state};
    use cratestack::axum::routing::get;
    use tower::ServiceExt;

    use super::*;
    use crate::schema::Cratestack;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(AUTHORIZATION, value.parse().unwrap());
        }
        map
    }

    #[test]
    fn a_bearer_token_is_exactly_one_bearer_header() {
        assert_eq!(
            bearer_token(&headers(&["Bearer abc.def.ghi"])),
            Ok("abc.def.ghi")
        );
        assert_eq!(bearer_token(&headers(&["bearer abc"])), Ok("abc"));
        assert_eq!(bearer_token(&headers(&["BEARER  abc "])), Ok("abc"));
        for refused in [
            headers(&[]),
            headers(&["Basic dXNlcjpwYXNz"]),
            headers(&["Bearer"]),
            headers(&["Bearer "]),
            headers(&["abc"]),
            headers(&["Bearer a", "Bearer b"]),
        ] {
            assert_eq!(
                bearer_token(&refused),
                Err(BearerRefusal::Malformed),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn no_door_at_all_is_refused_at_construction() {
        assert_eq!(ApiDoors::new(false, None).unwrap_err(), NoDoor);
        assert!(ApiDoors::new(true, None).is_ok());
    }

    /// Answers with a fixed outcome and counts the tokens it was asked about.
    struct Fixed(Result<VerifiedBearer, BearerRefusal>);

    #[async_trait::async_trait]
    impl BearerVerifier for Fixed {
        async fn verify(&self, _token: &str) -> Result<VerifiedBearer, BearerRefusal> {
            self.0.clone()
        }
    }

    fn lazy_directory() -> SignerDirectory {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        SignerDirectory::new(Cratestack::builder(pool).build())
    }

    /// The door over a handler that answers 200, with the request's signer
    /// recorded first when `signed` (what the envelope layer does).
    fn app(bearer: Option<Result<VerifiedBearer, BearerRefusal>>, signed: bool) -> Router {
        let doors = ApiDoors::new(
            true,
            bearer.map(|outcome| Arc::new(Fixed(outcome)) as Arc<dyn BearerVerifier>),
        )
        .unwrap();
        let state = BearerDoorState {
            doors,
            directory: lazy_directory(),
        };
        Router::new()
            .route("/probe", get(|| async { "reached the handler" }))
            .layer(from_fn_with_state(state, enforce_bearer_door))
            .layer(from_fn(
                move |mut request: Request, next: Next| async move {
                    if signed {
                        request.extensions_mut().insert(VerifiedSigner::new(
                            vec![0_u8; 8],
                            [7_u8; 32],
                            -19,
                        ));
                    }
                    next.run(request).await
                },
            ))
    }

    async fn call(app: Router, authorization: Option<&str>) -> (StatusCode, String) {
        let mut request = HttpRequest::builder().uri("/probe");
        if let Some(value) = authorization {
            request = request.header(AUTHORIZATION, value);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = cratestack::axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn ok_claims() -> VerifiedBearer {
        VerifiedBearer::Machine {
            client_id: "appc_demo".to_owned(),
            scope: None,
        }
    }

    /// One caller, one credential: a signed request carrying `Authorization` is
    /// a 400 in every configuration of the door, with or without a verifier, and
    /// never reaches the handler.
    #[tokio::test]
    async fn a_signed_request_carrying_authorization_is_a_400() {
        for bearer in [None, Some(Ok(ok_claims()))] {
            let (status, body) = call(app(bearer, true), Some("Bearer anything")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert!(
                body.contains("cannot also carry an Authorization header"),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn a_signed_request_without_authorization_passes_the_door() {
        let (status, body) = call(app(Some(Ok(ok_claims())), true), None).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, "reached the handler")
        );
    }

    /// Every refusal is the same coarse 401 as a forged signature, whichever
    /// check fired: the caller learns nothing about which.
    #[tokio::test]
    async fn every_bearer_refusal_is_the_same_coarse_401() {
        let all = [
            BearerRefusal::Malformed,
            BearerRefusal::MissingKid,
            BearerRefusal::UnknownKid,
            BearerRefusal::Signature,
            BearerRefusal::Issuer,
            BearerRefusal::Audience,
            BearerRefusal::Expired,
            BearerRefusal::NotYetValid,
            BearerRefusal::HumanToken,
            BearerRefusal::MachineToken,
            BearerRefusal::IdToken,
            BearerRefusal::SenderConstrained,
        ];
        let mut bodies = std::collections::BTreeSet::new();
        for refusal in all {
            let (status, body) = call(app(Some(Err(refusal)), false), Some("Bearer tok")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{refusal}");
            assert!(
                body.contains("request could not be authenticated"),
                "{refusal}: {body}"
            );
            bodies.insert(body);
        }
        assert_eq!(
            bodies.len(),
            1,
            "the body must not depend on the refusal: {bodies:?}"
        );
    }

    /// The one answer of the door that is not the coarse 401: a person whose
    /// role acts only from a device. [`error_response`] renders
    /// `Forbidden(DEVICE_REQUIRED)` as a 403 with that text and nothing else, and
    /// it is not the body a refused credential gets.
    #[tokio::test]
    async fn a_device_only_role_is_a_403_with_its_own_text() {
        let response = error_response(&CratestackError::Forbidden(
            crate::auth::DEVICE_REQUIRED.to_owned(),
        ));
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = cratestack::axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body).into_owned();
        assert!(body.contains(r#""code":"FORBIDDEN""#), "{body}");
        assert!(body.contains(crate::auth::DEVICE_REQUIRED), "{body}");
        for role in crate::auth::DEVICE_ONLY_ROLES {
            assert!(!body.contains(role), "the answer names no role: {body}");
        }
        assert!(
            !body.contains("request could not be authenticated"),
            "a 403 is not the coarse 401: {body}"
        );
    }

    /// Without a verifier (the signed door alone), or without a bearer header, an
    /// unsigned request is a 401 and the handler is never reached.
    #[tokio::test]
    async fn an_unsigned_request_without_a_verifiable_bearer_is_a_401() {
        for (bearer, authorization) in [
            (None, Some("Bearer tok")),
            (None, None),
            (Some(Ok(ok_claims())), None),
            (Some(Ok(ok_claims())), Some("Basic dXNlcjpwYXNz")),
        ] {
            let (status, body) = call(app(bearer.clone(), false), authorization).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{bearer:?} {authorization:?}"
            );
            assert!(
                body.contains("request could not be authenticated"),
                "{body}"
            );
        }
    }
}
