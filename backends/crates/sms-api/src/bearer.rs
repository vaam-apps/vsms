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
    /// The token belongs to a person (it carries an `identity`), not a machine.
    /// Only machine callers have a door here; a person signs in some other way
    /// (VSA-4).
    #[error("the token belongs to a person, not a machine client")]
    HumanToken,
}

/// What a verified token says, and nothing it does not.
///
/// A client id and the scopes it asked for: **claims that narrow, never grant**.
/// The caller's standing is read from the database by
/// [`SignerDirectory::bearer_standing`]; this struct is only how the token names
/// the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBearer {
    /// The token's `sub`, an `OauthClient.clientId`.
    pub client_id: String,
    /// The token's space-delimited `scope` claim, when it has one. The effective
    /// scope is its intersection with the client's own.
    pub scope: Option<String>,
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

/// Neither door open: `auth.methods` permits no way to say who a caller is. The
/// configuration refuses it first (`ConfigError::NoWayIn`); this is the same
/// refusal at the place that would otherwise build a gateway nobody can call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no way in: neither the signed envelope nor a machine token is accepted")]
pub struct NoDoor;

impl ApiDoors {
    /// `keypair`: `auth.methods` permits it. `bearer`: `Some` exactly when
    /// `machine_tokens` is **served** (permitted and `op:` written).
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
///    ([`SignerDirectory::bearer_standing`]: the `OauthClient` is active, its
///    `AppClient` live and the same client), and only then does the request
///    become a principal: `VerifiedPrincipal("client:<clientId>")`, the very
///    name the signed door gives the same client, so both doors draw from **one**
///    rate-limit bucket and **one** idempotency namespace.
///
/// A refusal is one 401 whatever failed; the [`BearerRefusal`] is written at
/// `debug`. A database failure while resolving the standing is a 5xx, never a
/// refusal.
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

    let Some(verifier) = state.doors.bearer.as_ref() else {
        return unauthenticated();
    };
    let verified = match authenticate_bearer(verifier.as_ref(), request.headers()).await {
        Ok(verified) => verified,
        Err(refusal) => {
            tracing::debug!(%refusal, "a bearer token was refused");
            return unauthenticated();
        }
    };
    let standing = match state.directory.bearer_standing(&verified).await {
        Ok(standing) => standing,
        Err(CratestackError::Unauthorized(_)) => {
            tracing::debug!(client_id = %verified.client_id, "a verified token's client has no standing");
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

    fn ok_claims() -> Result<VerifiedBearer, BearerRefusal> {
        Ok(VerifiedBearer {
            client_id: "appc_demo".to_owned(),
            scope: None,
        })
    }

    /// One caller, one credential: a signed request carrying `Authorization` is
    /// a 400 in every configuration of the door, with or without a verifier, and
    /// never reaches the handler.
    #[tokio::test]
    async fn a_signed_request_carrying_authorization_is_a_400() {
        for bearer in [None, Some(ok_claims())] {
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
        let (status, body) = call(app(Some(ok_claims()), true), None).await;
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

    /// Without a verifier (the signed door alone), or without a bearer header, an
    /// unsigned request is a 401 and the handler is never reached.
    #[tokio::test]
    async fn an_unsigned_request_without_a_verifiable_bearer_is_a_401() {
        for (bearer, authorization) in [
            (None, Some("Bearer tok")),
            (None, None),
            (Some(ok_claims()), None),
            (Some(ok_claims()), Some("Basic dXNlcjpwYXNz")),
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
