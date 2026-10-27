//! The server half of the transport, for the SDK's own tests.
//!
//! Built under `cfg(test)` and under the `responder` feature, never in the
//! module the npm package ships. It is what the gateway does to a sealed
//! request (find the operation, rebuild the binding the client signed, verify,
//! refuse a replay) and to its answer (seal it under the server key), written
//! against cratestack's own server envelope (`CoseEnvelope::server`), so the
//! SDK's suites run against a verifier that is not a copy of the SDK's sealing
//! code. The real `EnvelopeLayer` is exercised against the same sealing path
//! by the live test (`sdks/node/vsms-sdk-node/live/gateway.live.ts`) and by
//! `vsms-sdk-rust`'s `tests/live_gateway.rs`.

use std::borrow::Cow;
use std::sync::Arc;

use crate::contracts::{contract_digest, route_of};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use bytes::Bytes;
use cratestack::cose::{
    request_digest, CoseEnvelope, CoseMode, CoseVerifyKey, Ed25519Signer, StaticVerifierResolver,
};
use cratestack::{
    canonical_query, Binding, BoundHeaders, InMemoryNonceStore, PathParams, ResponseBinding,
    PAYLOAD_TYPE_HEADER,
};

const JSON: &str = "application/json";
const COSE_SIGN1: &str = "application/cose; cose-type=\"cose-sign1\"";

/// The gateway's envelope, for tests.
#[derive(Debug)]
pub struct Responder {
    server: CoseEnvelope,
    audience: String,
}

impl Responder {
    /// `server_seed` signs answers; `client_keys` are the registered public
    /// keys, as `ClientKey` (32 bytes, Ed25519) and `ConsoleKey` (65 bytes,
    /// SEC1 uncompressed, ESP256) rows hold them.
    ///
    /// # Errors
    ///
    /// A message when a key is not a point or the envelope cannot be built.
    pub fn new(
        server_seed: &[u8; 32],
        client_keys: &[Vec<u8>],
        audience: &str,
    ) -> Result<Self, String> {
        let mut resolver = StaticVerifierResolver::new();
        for key in client_keys {
            let key = match <[u8; 32]>::try_from(key.as_slice()) {
                Ok(ed25519) => CoseVerifyKey::ed25519(&ed25519),
                Err(_) => CoseVerifyKey::p256_sec1(key),
            }
            .map_err(|error| error.to_string())?;
            resolver = resolver.with_key(key);
        }
        let server = CoseEnvelope::server(
            CoseMode::Sign1,
            Arc::new(Ed25519Signer::from_seed(server_seed)),
            Arc::new(resolver),
            Arc::new(InMemoryNonceStore::new()),
        )
        .build()
        .map_err(|error| error.to_string())?;
        Ok(Self {
            server,
            audience: audience.to_owned(),
        })
    }

    /// A server seed's public key, which a client pins.
    ///
    /// # Errors
    ///
    /// Never in practice: an Ed25519 signer's verify key always has its bytes.
    pub fn public_key(seed: &[u8; 32]) -> Result<[u8; 32], String> {
        Ed25519Signer::from_seed(seed)
            .verify_key()
            .ed25519_bytes()
            .ok_or_else(|| "no public key".to_owned())
    }

    /// Opens one sealed request. `path` is the concrete path as received,
    /// `query` the query string as received and `headers` every request
    /// header as `(name, value)`. A request that does not verify, or that
    /// repeats one that did, is `Err(())`: the caller answers the unsigned
    /// `401`, and no more is known.
    ///
    /// # Errors
    ///
    /// `()` for a refused request, deliberately without a reason.
    #[allow(clippy::result_unit_err)]
    pub async fn open_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> Result<OpenedRequest, ()> {
        let (template, params) = route_of(method, path).ok_or(())?;
        let digest = contract_digest(method, &template).map_err(|_| ())?;
        let mut idempotency_key = None;
        let mut if_match = None;
        let mut payload_type = None;
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("idempotency-key") {
                idempotency_key = Some(value.clone());
            } else if name.eq_ignore_ascii_case("if-match") {
                if_match = Some(value.clone());
            } else if name.eq_ignore_ascii_case(PAYLOAD_TYPE_HEADER) {
                payload_type = Some(value.clone());
            }
        }
        // The client signs the *canonical* query (pairs sorted), so the server
        // must rebuild exactly that, whatever order the URL carried.
        let query = Some(canonical_query(query)).filter(|query| !query.is_empty());
        let binding = Binding {
            audience: Cow::Owned(self.audience.clone()),
            method: Cow::Owned(method.to_owned()),
            route: Cow::Owned(template),
            path_params: PathParams::Owned(params),
            query: query.map(Cow::Owned),
            contract_sha: digest,
            payload_media_type: Cow::Owned(payload_type.unwrap_or_else(|| JSON.to_owned())),
            bound_headers: BoundHeaders {
                idempotency_key: idempotency_key.clone().map(Cow::Owned),
                if_match: if_match.map(Cow::Owned),
            },
            response: None,
        };
        let request = Bytes::from(body);
        let opened = self
            .server
            .open_request(request.clone(), &binding)
            .await
            .map_err(|_| ())?;
        Ok(OpenedRequest {
            server: self.server.clone(),
            binding: binding.into_owned(),
            request,
            payload: String::from_utf8_lossy(&opened.payload).into_owned(),
            idempotency_key,
        })
    }
}

/// A request that verified, and the means to answer it.
#[derive(Debug)]
pub struct OpenedRequest {
    server: CoseEnvelope,
    binding: Binding<'static>,
    request: Bytes,
    /// The request payload as text (the JSON body; empty for a read).
    pub payload: String,
    /// The bound `Idempotency-Key`, if the request carried one.
    pub idempotency_key: Option<String>,
}

impl OpenedRequest {
    /// The route template the request was bound to.
    #[must_use]
    pub fn route(&self) -> &str {
        &self.binding.route
    }

    /// The decoded path parameters the request was bound to.
    #[must_use]
    pub fn path_params(&self) -> Vec<String> {
        self.binding.path_params.iter().map(str::to_owned).collect()
    }

    /// Seals `body` (JSON text) as the answer with `status`; returns the
    /// headers to send and the COSE message.
    ///
    /// # Errors
    ///
    /// The envelope's message when the server key cannot sign.
    pub async fn seal_response(
        &self,
        status: u16,
        body: &str,
    ) -> Result<(Vec<(String, String)>, Vec<u8>), String> {
        let mut binding = self.binding.clone();
        binding.payload_media_type = Cow::Borrowed(JSON);
        binding.response = Some(ResponseBinding {
            request: request_digest(&self.request),
            status,
        });
        let sealed = self
            .server
            .seal_response(body.as_bytes(), &binding)
            .await
            .map_err(|error| error.to_string())?;
        let headers = vec![
            ("content-type".to_owned(), COSE_SIGN1.to_owned()),
            (PAYLOAD_TYPE_HEADER.to_owned(), JSON.to_owned()),
        ];
        Ok((headers, sealed.to_vec()))
    }
}

/// 32 bytes of unpadded base64url, for the JavaScript side's keys.
///
/// # Errors
///
/// A message that never quotes the text.
pub fn key32(text: &str) -> Result<[u8; 32], String> {
    URL_SAFE_NO_PAD
        .decode(text.trim())
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
        .ok_or_else(|| "a key must be 32 bytes of unpadded base64url".to_owned())
}
