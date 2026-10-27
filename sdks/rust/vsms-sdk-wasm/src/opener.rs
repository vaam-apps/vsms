//! Opening the answer to a request somebody else sealed.
//!
//! A [`crate::Sealer`] opens an answer with the state its own `sealCall` kept:
//! the binding it signed and the digest of the bytes it sent. A server that
//! only *forwards* a browser's sealed request (the operator console's composer
//! route) holds neither, and still has to believe the gateway's answer before
//! it spends a key on it: a `200` from a socket proves nothing about who is on
//! the other end of it. [`Opener`] rebuilds that state from what the forwarder
//! has, the request's route, its query and its sealed bytes, and verifies the
//! answer under the same two pinned server keys, through the same cratestack
//! envelope, so there is one definition of "this answer is the gateway's, to
//! this request".
//!
//! It holds no signing key. The envelope a client verifies with insists on a
//! signer, so it is given one that refuses every request to sign.

use std::borrow::Cow;
use std::sync::Arc;

use bytes::Bytes;
use cratestack::cose::{
    request_digest, CoseEnvelope, CoseMode, CoseSigner, CoseVerifyKey, ExternalSigner,
    StaticVerifierResolver,
};
use cratestack::{
    canonical_query, Binding, BoundHeaders, CratestackError, PathParams, ResponseBinding,
    PAYLOAD_TYPE_HEADER,
};

use crate::contracts::{contract_digest, route_of};
use crate::core::{OpenError, SealError, ServerKeys, AUDIENCE, PAYLOAD_TYPE};

/// Verifies answers to requests it did not seal.
#[derive(Debug)]
pub struct Opener {
    cose: CoseEnvelope,
}

impl Opener {
    /// An opener that believes `server`.
    ///
    /// # Errors
    ///
    /// [`SealError::Seal`] when a key is not a point or the envelope cannot be
    /// built.
    pub fn new(server: &ServerKeys) -> Result<Self, SealError> {
        let invalid = |_| SealError::Seal("a pinned server key is invalid".into());
        let mut resolver = StaticVerifierResolver::new();
        for key in [&server.current, &server.next] {
            resolver = resolver.with_key(CoseVerifyKey::ed25519(key).map_err(invalid)?);
        }
        // Any point will do for the signer's identity: it is never asked to sign.
        let signer = ExternalSigner::ed25519(&server.current, |_tbs| async {
            Err::<Vec<u8>, _>(CratestackError::Internal(
                "an Opener holds no signing key".to_owned(),
            ))
        })
        .map_err(|error| SealError::Seal(error.to_string()))?;
        #[allow(
            clippy::arc_with_non_send_sync,
            reason = "on wasm32 cratestack's MaybeSend is empty and the module is single-threaded, \
                      so the Arc never crosses a thread; on a host the signer is Send + Sync and the \
                      lint does not fire, which is why this is an `allow` and not an `expect`"
        )]
        let signer: Arc<dyn CoseSigner> = Arc::new(signer);
        let cose = CoseEnvelope::client(CoseMode::Sign1, signer, Arc::new(resolver))
            .build()
            .map_err(|error| SealError::Seal(error.to_string()))?;
        Ok(Self { cose })
    }

    /// The payload of the answer (`status`, `headers`, `body`) to `request`, a
    /// sealed `method path?query` whose bytes are `request` and whose
    /// `Idempotency-Key` and `If-Match` were `request_headers` (the two a seal
    /// binds; any other header is ignored).
    ///
    /// The answer is believed only when it is a COSE message that verifies
    /// under a pinned server key **and** under this request: its route,
    /// parameters, query, operation, status and the digest of `request`. An
    /// answer sealed for another request, or for another status, is
    /// [`OpenError::Unverified`]; one that is not sealed at all (the gateway's
    /// coarse `401`, a proxy's page) is [`OpenError::Unsigned`] and nothing in
    /// its body is read.
    ///
    /// # Errors
    ///
    /// [`OpenError`].
    #[allow(clippy::too_many_arguments)]
    pub async fn open_answer(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        request: &[u8],
        request_headers: &[(String, String)],
        status: u16,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<Vec<u8>, OpenError> {
        let (template, params) = route_of(method, path).ok_or(OpenError::Unverified)?;
        let digest = contract_digest(method, &template).map_err(|_| OpenError::Unverified)?;
        if !is_cose(headers) {
            return Err(OpenError::Unsigned { status });
        }
        // The answer names the type it is sealed under; the call asked for one.
        let named = payload_type(headers)?;
        if named != PAYLOAD_TYPE {
            return Err(OpenError::Other(format!(
                "the answer names the payload type {named:?}, which this call did not ask for"
            )));
        }
        let pick = |name: &str| {
            request_headers
                .iter()
                .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
                .map(|(_, value)| Cow::Owned(value.clone()))
        };
        let query = Some(canonical_query(query)).filter(|query| !query.is_empty());
        let binding = Binding {
            audience: Cow::Borrowed(AUDIENCE),
            method: Cow::Borrowed(method),
            route: Cow::Owned(template),
            path_params: PathParams::Owned(params),
            query: query.map(Cow::Owned),
            contract_sha: digest,
            payload_media_type: Cow::Borrowed(PAYLOAD_TYPE),
            bound_headers: BoundHeaders {
                idempotency_key: pick("idempotency-key"),
                if_match: pick("if-match"),
            },
            response: Some(ResponseBinding {
                request: request_digest(request),
                status,
            }),
        };
        match self.cose.open_response(Bytes::from(body), &binding).await {
            Ok(opened) => Ok(opened.payload.to_vec()),
            Err(CratestackError::Unauthorized(_)) => Err(OpenError::Unverified),
            Err(other) => Err(OpenError::Other(other.to_string())),
        }
    }
}

/// Whether the answer names exactly one content type, an `application/cose`
/// one.
fn is_cose(headers: &[(String, String)]) -> bool {
    let mut types = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"));
    match (types.next(), types.next()) {
        (Some((_, value)), None) => value
            .split(';')
            .next()
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/cose")),
        _ => false,
    }
}

/// The payload type the answer names. Absent means CBOR, which a vsms call
/// never asks for, so it is returned as such and fails the comparison.
fn payload_type(headers: &[(String, String)]) -> Result<&str, OpenError> {
    let mut named = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(PAYLOAD_TYPE_HEADER));
    match (named.next(), named.next()) {
        (None, _) => Ok("application/cbor"),
        (Some((_, value)), None) => Ok(value.as_str()),
        (Some(_), Some(_)) => Err(OpenError::Other(
            "the answer names its payload type more than once".to_owned(),
        )),
    }
}
