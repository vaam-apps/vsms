//! The JavaScript surface of [`crate::responder`]: `Responder` and
//! `OpenedRequest`, for the Node SDK's tests only (the `responder` feature).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

use crate::responder::{key32, seal_delivery, OpenedRequest, Responder};

fn failure(message: &str) -> JsValue {
    JsValue::from_str(message)
}

/// The gateway's envelope, for tests.
#[wasm_bindgen(js_name = "Responder")]
#[derive(Debug)]
pub struct JsResponder {
    inner: Responder,
}

#[wasm_bindgen(js_class = "Responder")]
impl JsResponder {
    /// `server_seed` signs answers; `client_keys` are the registered public
    /// keys (unpadded base64url): 32 bytes for an Ed25519 `ClientKey`, 65 for an
    /// ESP256 `ConsoleKey`. `audience` is what the server expects requests to
    /// be addressed to (the gateway's is `vsms`).
    #[wasm_bindgen(constructor)]
    pub fn new(
        server_seed: &str,
        client_keys: Array,
        audience: &str,
    ) -> Result<JsResponder, JsValue> {
        let seed = key32(server_seed).map_err(|e| failure(&e))?;
        let mut keys = Vec::new();
        for key in client_keys.iter() {
            let text = key
                .as_string()
                .ok_or_else(|| failure("client keys must be strings"))?;
            keys.push(
                URL_SAFE_NO_PAD
                    .decode(text.trim())
                    .map_err(|_| failure("a client key must be unpadded base64url"))?,
            );
        }
        Ok(Self {
            inner: Responder::new(&seed, &keys, audience).map_err(|e| failure(&e))?,
        })
    }

    /// The server's public key, unpadded base64url: what a client pins.
    #[wasm_bindgen(js_name = "serverPublicKey")]
    pub fn server_public_key(seed: &str) -> Result<String, JsValue> {
        let seed = key32(seed).map_err(|e| failure(&e))?;
        Responder::public_key(&seed)
            .map(|key| URL_SAFE_NO_PAD.encode(key))
            .map_err(|e| failure(&e))
    }

    /// Opens one sealed request. `path` is the concrete path as received;
    /// `headers` is the request's `[name, value]` pairs. Rejects with the
    /// coarse text `refused` when the message does not verify: the caller
    /// answers an unsigned `401`.
    #[wasm_bindgen(js_name = "openRequest")]
    pub async fn open_request(
        &self,
        method: String,
        path: String,
        query: Option<String>,
        headers: Array,
        body: Uint8Array,
    ) -> Result<JsOpenedRequest, JsValue> {
        let mut pairs = Vec::new();
        for entry in headers.iter() {
            let pair = Array::from(&entry);
            if let (Some(name), Some(value)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
                pairs.push((name, value));
            }
        }
        let opened = self
            .inner
            .open_request(&method, &path, query.as_deref(), &pairs, body.to_vec())
            .await
            .map_err(|()| failure("refused"))?;
        Ok(JsOpenedRequest { inner: opened })
    }
}

/// A request that verified, and the means to answer it.
#[wasm_bindgen(js_name = "OpenedRequest")]
#[derive(Debug)]
pub struct JsOpenedRequest {
    inner: OpenedRequest,
}

#[wasm_bindgen(js_class = "OpenedRequest")]
impl JsOpenedRequest {
    /// The request payload, as text (the JSON body; empty for a read).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn payload(&self) -> String {
        self.inner.payload.clone()
    }

    /// The bound `Idempotency-Key`, if the request carried one.
    #[wasm_bindgen(getter, js_name = "idempotencyKey")]
    #[must_use]
    pub fn idempotency_key(&self) -> Option<String> {
        self.inner.idempotency_key.clone()
    }

    /// The route template the request was bound to.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn route(&self) -> String {
        self.inner.route().to_owned()
    }

    /// The decoded path parameters the request was bound to.
    #[wasm_bindgen(getter, js_name = "pathParams")]
    #[must_use]
    pub fn path_params(&self) -> Vec<String> {
        self.inner.path_params()
    }

    /// Seals `body` (JSON text) as the answer with `status`; returns
    /// `{ headers: [name, value][], body: Uint8Array }`.
    #[wasm_bindgen(js_name = "sealResponse")]
    pub async fn seal_response(&self, status: u16, body: String) -> Result<JsValue, JsValue> {
        let (headers, sealed) = self
            .inner
            .seal_response(status, &body)
            .await
            .map_err(|e| failure(&e))?;
        let pairs = Array::new();
        for (name, value) in headers {
            let pair = Array::new();
            pair.push(&name.into());
            pair.push(&value.into());
            pairs.push(&pair);
        }
        let out = Object::new();
        let _ = Reflect::set(&out, &"headers".into(), &pairs);
        let _ = Reflect::set(&out, &"body".into(), &Uint8Array::from(sealed.as_slice()));
        Ok(out.into())
    }
}

/// A webhook delivery as the gateway's worker seals it, signed with the seed of
/// the pinned server key. `contract_hex` (64 hex characters) replaces the
/// delivery contract digest, to test that a receiver refuses another one;
/// `iat_seconds` pins the timestamp. For tests only.
#[wasm_bindgen(js_name = "sealDelivery")]
#[allow(
    unreachable_pub,
    reason = "wasm-bindgen exports it from this private module; the structs beside it are `pub` for the same reason"
)]
pub async fn js_seal_delivery(
    seed: String,
    audience: String,
    route: String,
    body: String,
    contract_hex: Option<String>,
    iat_seconds: Option<f64>,
) -> Result<Uint8Array, JsValue> {
    let seed = key32(&seed).map_err(|e| failure(&e))?;
    let contract = match contract_hex {
        None => None,
        Some(hex) => {
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|at| {
                    hex.get(at..at + 2)
                        .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                })
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| failure("contractHex must be hex"))?;
            Some(<[u8; 32]>::try_from(bytes).map_err(|_| failure("contractHex must be 32 bytes"))?)
        }
    };
    #[allow(clippy::cast_possible_truncation)]
    let iat = iat_seconds.map(|seconds| seconds as i64);
    seal_delivery(&seed, &audience, &route, contract, &body, iat)
        .await
        .map(|bytes| Uint8Array::from(bytes.as_slice()))
        .map_err(|e| failure(&e))
}
