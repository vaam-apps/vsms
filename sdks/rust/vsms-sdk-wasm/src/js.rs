//! The JavaScript surface: `Sealer` and `SealedCall`, over [`crate::core`].
//!
//! Errors cross as a plain object `{ kind, message, status? }` with `kind` one
//! of `"unbound" | "seal" | "unsigned" | "unverified" | "unreadable" |
//! "invalid"`, so the TypeScript side maps them to its own error classes
//! without parsing text. `unbound` and `seal` are refusals **before anything
//! is sent**; `unsigned`, `unverified` and `unreadable` are about an answer
//! that already arrived, so the request *was* sent.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use cratestack::cose::{CoseSigner, ExternalSigner};
use cratestack::CratestackError;
use js_sys::{Array, ArrayBuffer, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use zeroize::{Zeroize as _, Zeroizing};

use crate::contracts::route_of;
use crate::core::{Bound, OpenError, SealError, Sealed, Sealer, SealerKey, ServerKeys};
use crate::opener::Opener;

fn error(kind: &str, message: &str, status: Option<u16>) -> JsValue {
    let object = Object::new();
    // `Reflect::set` on a fresh plain object cannot fail.
    let _ = Reflect::set(&object, &"kind".into(), &kind.into());
    let _ = Reflect::set(&object, &"message".into(), &message.into());
    if let Some(status) = status {
        let _ = Reflect::set(&object, &"status".into(), &JsValue::from(status));
    }
    object.into()
}

fn invalid(message: &str) -> JsValue {
    error("invalid", message, None)
}

fn seal_error(error_: &SealError) -> JsValue {
    match error_ {
        SealError::Unbound(message) => error("unbound", message, None),
        SealError::Seal(message) => error("seal", message, None),
    }
}

fn open_error(error_: &OpenError) -> JsValue {
    match error_ {
        OpenError::Unsigned { status } => error("unsigned", &error_.to_string(), Some(*status)),
        OpenError::Unverified => error("unverified", &error_.to_string(), None),
        // The request went out; what came back could not be read. Never
        // `seal`, which promises that nothing was sent.
        OpenError::Other(message) => error("unreadable", message, None),
    }
}

/// Unpadded base64url of exactly `len` bytes, whitespace ignored. The text is
/// never echoed: it may be a private seed.
fn bytes_from_base64url(text: &str, what: &str, len: usize) -> Result<Zeroizing<Vec<u8>>, JsValue> {
    let wrong = || invalid(&format!("{what} must be {len} bytes of unpadded base64url"));
    // Wiped when dropped: the bytes may be a seed, decoded or not.
    let decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(text.trim()).map_err(|_| wrong())?);
    if decoded.len() == len {
        Ok(decoded)
    } else {
        Err(wrong())
    }
}

pub(crate) fn key_from_base64url(text: &str, what: &str) -> Result<[u8; 32], JsValue> {
    bytes_from_base64url(text, what, 32).and_then(|bytes| {
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| invalid(&format!("{what} must be 32 bytes")))
    })
}

fn pairs_from_js(headers: &JsValue) -> Result<Vec<(String, String)>, JsValue> {
    let entries = Array::from(headers);
    let mut out = Vec::with_capacity(entries.length() as usize);
    for entry in entries.iter() {
        let pair = Array::from(&entry);
        match (pair.get(0).as_string(), pair.get(1).as_string()) {
            (Some(name), Some(value)) => out.push((name, value)),
            _ => {
                return Err(invalid(
                    "headers must be an array of [name, value] string pairs",
                ));
            }
        }
    }
    Ok(out)
}

fn pairs_to_js(pairs: &[(String, String)]) -> JsValue {
    pairs
        .iter()
        .map(|(name, value)| {
            let pair = Array::new();
            pair.push(&name.into());
            pair.push(&value.into());
            JsValue::from(pair)
        })
        .collect::<Array>()
        .into()
}

type Signing = Pin<Box<dyn Future<Output = Result<Vec<u8>, CratestackError>>>>;

/// The most a signing callback may hand back. A raw Ed25519 or P-256 signature
/// is 64 bytes and a DER ECDSA one at most 72; anything near this bound is not
/// a signature, and is refused before it is copied into this module.
const MAX_SIGNATURE_BYTES: u32 = 128;

/// The bytes of `value` if it is a `Uint8Array` or an `ArrayBuffer` of a
/// plausible signature size, and `None` for anything else.
///
/// A callback is the caller's code and may return anything: a number, a
/// string, an object with a throwing getter. A `Uint8Array::new(&value)` over
/// such a value throws a `RangeError` (or worse, tries to allocate it) from
/// inside a future, where it is an uncaught exception: it kills a default
/// Node process, and it leaves this module's task queue marked as running, so
/// every later call on every sealer hangs. So the value is type-checked first
/// (`instanceof` cannot throw through wasm-bindgen's binding) and copied
/// through `Reflect::construct`, whose failure (a detached buffer) comes back
/// as a `Result`. The copy is also what makes a `Uint8Array` subclass's own
/// `length` getter irrelevant: `new Uint8Array(typedArray)` reads internal
/// slots.
fn signature_bytes(value: &JsValue) -> Option<Vec<u8>> {
    if !value.is_instance_of::<Uint8Array>() && !value.is_instance_of::<ArrayBuffer>() {
        return None;
    }
    let constructor: Function = Reflect::get(&js_sys::global(), &"Uint8Array".into())
        .ok()?
        .dyn_into()
        .ok()?;
    let copy: Uint8Array = Reflect::construct(&constructor, &Array::of1(value))
        .ok()?
        .dyn_into()
        .ok()?;
    (copy.length() <= MAX_SIGNATURE_BYTES).then(|| copy.to_vec())
}

/// The future-returning callback `ExternalSigner::{ed25519, esp256}` take, over
/// a JS `(tbs: Uint8Array) => Uint8Array | Promise<Uint8Array>`.
fn callback(sign: Function) -> impl Fn(Vec<u8>) -> Signing {
    move |tbs: Vec<u8>| {
        let sign = sign.clone();
        Box::pin(async move {
            let failed = |what: &str| CratestackError::Internal(what.to_owned());
            let returned = sign
                .call1(&JsValue::NULL, &Uint8Array::from(tbs.as_slice()))
                .map_err(|_| failed("the signing callback threw"))?;
            let settled = JsFuture::from(Promise::resolve(&returned))
                .await
                .map_err(|_| failed("the signing callback rejected"))?;
            signature_bytes(&settled).ok_or_else(|| {
                failed("the signing callback must return the signature as a Uint8Array")
            })
        })
    }
}

#[expect(
    clippy::arc_with_non_send_sync,
    reason = "a JS callback's future is not Send, and on wasm32 cratestack's MaybeSend is empty: \
              the module is single-threaded, so the Arc never crosses one"
)]
fn shared(signer: ExternalSigner) -> Arc<dyn CoseSigner> {
    Arc::new(signer)
}

fn server_keys(current: &str, next: &str) -> Result<ServerKeys, JsValue> {
    ServerKeys::new(
        key_from_base64url(current, "the current server key")?,
        key_from_base64url(next, "the next server key")?,
    )
    .map_err(|e| seal_error(&e))
}

/// A caller's sealing identity. See the crate docs for the flow.
#[wasm_bindgen(js_name = "Sealer")]
#[derive(Debug)]
pub struct JsSealer {
    inner: Sealer,
}

#[wasm_bindgen(js_class = "Sealer")]
impl JsSealer {
    /// Signs with the Ed25519 key whose seed is `seed_base64url` (32 bytes,
    /// unpadded base64url). `server_current` and `server_next` are the pinned
    /// response keys in the same spelling; both are required and differ.
    #[wasm_bindgen(js_name = "fromSeed")]
    pub fn from_seed(
        seed_base64url: &str,
        server_current: &str,
        server_next: &str,
    ) -> Result<JsSealer, JsValue> {
        let mut seed = key_from_base64url(seed_base64url, "the seed")?;
        let key = SealerKey::Seed(seed);
        seed.zeroize();
        Self::build(key, server_current, server_next)
    }

    /// Signs through `sign`, a callback `(tbs: Uint8Array) => Uint8Array |
    /// Promise<Uint8Array>` returning the 64-byte pure-Ed25519 signature over
    /// exactly `tbs` (`crypto.sign(null, tbs, key)`). The key never enters this
    /// module. A signature that does not verify under `public_key_base64url` is
    /// refused here.
    #[wasm_bindgen(js_name = "fromCallback")]
    pub fn from_callback(
        public_key_base64url: &str,
        sign: Function,
        server_current: &str,
        server_next: &str,
    ) -> Result<JsSealer, JsValue> {
        let public_key = key_from_base64url(public_key_base64url, "the public key")?;
        let signer = ExternalSigner::ed25519(&public_key, callback(sign))
            .map_err(|_| invalid("publicKey is not an Ed25519 public key"))?;
        Self::build(
            SealerKey::Ed25519 {
                public_key,
                signer: shared(signer),
            },
            server_current,
            server_next,
        )
    }

    /// Signs with a P-256 key behind `sign`, the operator console's WebCrypto
    /// non-extractable key: `(tbs: Uint8Array) => Uint8Array |
    /// Promise<Uint8Array>` returning an ECDSA-over-SHA-256 signature of
    /// exactly `tbs`, raw 64-byte `r || s` (what `crypto.subtle.sign` returns)
    /// or DER. The bytes are hashed by the signature scheme, so pass `tbs` as
    /// given and never a digest. `public_key_base64url` is the SEC1 point
    /// (`crypto.subtle.exportKey("raw", publicKey)`), 65 bytes uncompressed
    /// or 33 compressed.
    #[wasm_bindgen(js_name = "fromEsp256Callback")]
    pub fn from_esp256_callback(
        public_key_base64url: &str,
        sign: Function,
        server_current: &str,
        server_next: &str,
    ) -> Result<JsSealer, JsValue> {
        let text = public_key_base64url.trim();
        let public_key = URL_SAFE_NO_PAD
            .decode(text)
            .map_err(|_| invalid("the public key must be unpadded base64url"))?;
        let signer = ExternalSigner::esp256(&public_key, callback(sign))
            .map_err(|_| invalid("publicKey is not a P-256 public key"))?;
        Self::build(
            SealerKey::Esp256 {
                public_key,
                signer: shared(signer),
            },
            server_current,
            server_next,
        )
    }

    fn build(key: SealerKey, server_current: &str, server_next: &str) -> Result<JsSealer, JsValue> {
        let keys = server_keys(server_current, server_next)?;
        let inner = Sealer::new(key, &keys).map_err(|e| seal_error(&e))?;
        Ok(JsSealer { inner })
    }

    /// The public half, unpadded base64url: what `register-client
    /// --public-key` (Ed25519, 32 bytes) or `registerConsoleKey` (ESP256, 65
    /// bytes SEC1 uncompressed) takes.
    #[wasm_bindgen(getter, js_name = "publicKey")]
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.inner.public_key())
    }

    /// `"ed25519"` or `"esp256"`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn algorithm(&self) -> String {
        self.inner.algorithm().as_str().to_owned()
    }

    /// Seals one call. `route` is the schema's template (`/messages/{id}`),
    /// `params` its decoded parameters in order, `body` the JSON text of a
    /// write, `query` the query string as it will be sent, `idempotencyKey` and
    /// `ifMatch` the headers of the same names as they will be sent (a strong
    /// ETag, quotes included).
    #[wasm_bindgen(js_name = "sealCall")]
    #[allow(
        clippy::too_many_arguments,
        reason = "the JavaScript surface is positional, and the two bound headers are optional trailing arguments so the Node SDK's calls are unchanged"
    )]
    pub async fn seal_call(
        &self,
        method: String,
        route: String,
        params: Vec<String>,
        body: Option<String>,
        query: Option<String>,
        idempotency_key: Option<String>,
        if_match: Option<String>,
    ) -> Result<JsSealedCall, JsValue> {
        let params: Vec<&str> = params.iter().map(String::as_str).collect();
        let sealed = self
            .inner
            .seal(
                &method,
                &route,
                &params,
                body.as_deref(),
                query.as_deref(),
                &Bound {
                    idempotency_key: idempotency_key.as_deref(),
                    if_match: if_match.as_deref(),
                },
            )
            .await
            .map_err(|e| seal_error(&e))?;
        Ok(JsSealedCall { inner: sealed })
    }
}

/// The route a concrete path is: `{ route, params }`, to be handed to
/// `Sealer.sealCall` as given. For a caller that holds a path rather than a
/// template (the operator console's `fetch`-shaped data layer). `path` carries
/// no query. A path the schema does not serve is an `unbound` failure, before
/// anything is signed.
#[wasm_bindgen(js_name = "routeOf")]
#[allow(
    unreachable_pub,
    reason = "wasm-bindgen exports it from this private module; the structs beside it are `pub` for the same reason"
)]
pub fn route_of_js(method: &str, path: &str) -> Result<JsValue, JsValue> {
    let (route, params) = route_of(method, path).ok_or_else(|| {
        error(
            "unbound",
            &format!("the schema serves no operation `{method} {path}`"),
            None,
        )
    })?;
    let found = Object::new();
    // `Reflect::set` on a fresh plain object cannot fail.
    let _ = Reflect::set(&found, &"route".into(), &route.into());
    let params: Array = params.into_iter().map(JsValue::from).collect();
    let _ = Reflect::set(&found, &"params".into(), &params.into());
    Ok(found.into())
}

/// One sealed call: send `body` with `headers` to `target`, then
/// `openResponse` the answer.
#[wasm_bindgen(js_name = "SealedCall")]
#[derive(Debug)]
pub struct JsSealedCall {
    inner: Sealed,
}

#[wasm_bindgen(js_class = "SealedCall")]
impl JsSealedCall {
    /// The request body: the COSE message.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn body(&self) -> Uint8Array {
        Uint8Array::from(self.inner.body.as_slice())
    }

    /// The headers to send as given, as `[name, value]` pairs.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn headers(&self) -> JsValue {
        pairs_to_js(&self.inner.headers)
    }

    /// Where to send it, relative to the gateway's origin: the path with its
    /// parameters in place, and `?query` if there was one.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn target(&self) -> String {
        self.inner.target.clone()
    }

    /// Verifies the answer and returns its payload as text. `headers` is the
    /// response's header list as `[name, value]` pairs.
    #[wasm_bindgen(js_name = "openResponse")]
    pub async fn open_response(
        &mut self,
        status: u16,
        headers: JsValue,
        body: Uint8Array,
    ) -> Result<String, JsValue> {
        let pairs = pairs_from_js(&headers)?;
        let payload = self
            .inner
            .open_response(status, &pairs, body.to_vec())
            .await
            .map_err(|e| open_error(&e))?;
        String::from_utf8(payload).map_err(|_| error("unreadable", "the answer is not UTF-8", None))
    }
}

/// Verifies the answer to a request somebody else sealed: the console's
/// server forwards a browser's sealed bytes and believes the gateway's answer
/// only if it opens here, under the two pinned server keys and against those
/// very bytes. It holds no signing key.
#[wasm_bindgen(js_name = "Opener")]
#[derive(Debug)]
pub struct JsOpener {
    inner: Opener,
}

#[wasm_bindgen(js_class = "Opener")]
impl JsOpener {
    /// An opener that believes `server_current` and `server_next` (unpadded
    /// base64url, both required and distinct).
    #[wasm_bindgen(js_name = "forServerKeys")]
    pub fn for_server_keys(server_current: &str, server_next: &str) -> Result<JsOpener, JsValue> {
        let keys = server_keys(server_current, server_next)?;
        let inner = Opener::new(&keys).map_err(|e| seal_error(&e))?;
        Ok(JsOpener { inner })
    }

    /// The payload, as text, of the answer (`status`, `headers`, `body`) to
    /// the request `method path?query` whose sealed bytes are `request` and
    /// whose `Idempotency-Key` and `If-Match` were among `requestHeaders`
    /// (`[name, value]` pairs; any other header is ignored). Rejects with an
    /// `unsigned`, `unverified` or `unreadable` failure, as
    /// `SealedCall.openResponse` does.
    #[wasm_bindgen(js_name = "openAnswer")]
    #[allow(
        clippy::too_many_arguments,
        reason = "the JavaScript surface is positional"
    )]
    pub async fn open_answer(
        &self,
        method: String,
        path: String,
        query: Option<String>,
        request: Uint8Array,
        request_headers: JsValue,
        status: u16,
        headers: JsValue,
        body: Uint8Array,
    ) -> Result<String, JsValue> {
        let request_headers = pairs_from_js(&request_headers)?;
        let headers = pairs_from_js(&headers)?;
        let payload = self
            .inner
            .open_answer(
                &method,
                &path,
                query.as_deref(),
                &request.to_vec(),
                &request_headers,
                status,
                &headers,
                body.to_vec(),
            )
            .await
            .map_err(|e| open_error(&e))?;
        String::from_utf8(payload).map_err(|_| error("unreadable", "the answer is not UTF-8", None))
    }
}
