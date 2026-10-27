//! A webhook receiver for the live suites: a real cratestack `EnvelopeLayer`
//! (`Required`) in front of an axum router, configured the way vaam configures
//! `/webhooks/vsms` (VA-5).
//!
//! A suite that wants to know "would a receiver accept what the worker sent?"
//! hands the request the worker produced (its body and headers, as `wiremock`
//! recorded them) to [`WebhookReceiver::deliver`] and asks for the verdict. The
//! layer is the framework's, the router is axum's, and the only thing written
//! here is the `BindingResolver` that names the receiver's literal `POST`
//! routes, exactly what `sms-webhook`'s `lib.md` tells a receiver to write.
//!
//! It knows neither `sms-webhook` nor `sms-api` (both dev-depend on this
//! crate): the caller passes the contract digest, the audience and vsms's
//! public keys.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request};
use axum::routing::post;
use cratestack::cose::{
    CoseEnvelope, CoseMode, CoseVerifyKey, Ed25519Signer, StaticVerifierResolver,
};
use cratestack::envelope_layer::{
    BindingResolver, EnvelopeLayer, EnvelopeMode, Resolution, ResolvedRoute, RouteRequest,
};
use cratestack::{AcceptedContracts, InMemoryNonceStore};
use tower::ServiceExt as _;

/// The only payload type a delivery is sealed under.
const PAYLOAD_TYPE: &str = "application/json";

/// What the receiver's layer said to one delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The HTTP status. `200` is an accepted delivery; `401` is the layer's one
    /// coarse refusal (wrong audience, route, key, nonce, clock or bytes);
    /// `426` is a contract the receiver does not declare.
    pub status: u16,
    /// Whether the answer is a sealed message (`application/cose`).
    pub sealed_answer: bool,
}

struct Routes(Vec<&'static str>);

impl BindingResolver for Routes {
    fn resolve(&self, request: &RouteRequest<'_>) -> Resolution {
        match request.matched_path() {
            Some(path) if request.method() == Method::POST && self.0.contains(&path) => {
                Resolution::Op(ResolvedRoute::new(path.to_owned(), Vec::new()))
            }
            _ => Resolution::Unresolved,
        }
    }
}

/// An `EnvelopeLayer` receiver on one or more literal `POST` routes.
#[derive(Clone)]
pub struct WebhookReceiver {
    router: Router,
    seen: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for WebhookReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookReceiver").finish_non_exhaustive()
    }
}

impl WebhookReceiver {
    /// A receiver that trusts `vsms_keys` (the `current` and `next` public keys
    /// `sms-gateway server-keys` prints), answers for `audience`, serves each of
    /// `routes` and declares `contract` for all of them.
    ///
    /// # Panics
    ///
    /// On an unusable key or an empty audience: a test misconfiguration.
    #[must_use]
    pub fn new(
        vsms_keys: &[[u8; 32]],
        audience: &str,
        routes: &[&'static str],
        contract: [u8; 32],
    ) -> Self {
        let mut resolver = StaticVerifierResolver::new();
        for key in vsms_keys {
            resolver = resolver.with_key(CoseVerifyKey::ed25519(key).expect("an Ed25519 key"));
        }
        let envelope = CoseEnvelope::server(
            CoseMode::Sign1,
            // The receiver's own key, which seals its answers: unrelated to vsms's.
            Arc::new(Ed25519Signer::from_seed(&[0x5e; 32])),
            Arc::new(resolver),
            Arc::new(InMemoryNonceStore::new()),
        )
        .build()
        .expect("a server envelope");

        // `AcceptedContracts` is `&'static`; a test receiver is built a handful of
        // times per process, so leaking its table is the honest cost.
        let digests: &'static [[u8; 32]] = Box::leak(Box::new([contract]));
        let rows: Vec<(&'static str, &'static [[u8; 32]])> = routes
            .iter()
            .map(|route| {
                let key: &'static str = Box::leak(format!("POST {route}").into_boxed_str());
                (key, digests)
            })
            .collect();
        let accepted: AcceptedContracts = Box::leak(rows.into_boxed_slice());

        let layer = EnvelopeLayer::builder(envelope, audience, accepted)
            .binding_resolver(Routes(routes.to_vec()))
            .policy(EnvelopeMode::Required)
            .payload_media_types([PAYLOAD_TYPE], [PAYLOAD_TYPE])
            .build()
            .expect("an envelope layer");

        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut app = Router::new();
        for route in routes {
            let seen = seen.clone();
            app = app.route(
                route,
                post(move |body: String| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().expect("seen bodies").push(body);
                        // JSON, named as such: the layer seals only the payload
                        // types it negotiated, and a `text/plain` body is a sealed 500.
                        ([("content-type", "application/json")], "{}")
                    }
                }),
            );
        }
        Self {
            router: app.layer(layer),
            seen,
        }
    }

    /// Offers a delivery (`headers` and `body` exactly as they went on the wire)
    /// to the receiver at `route`.
    ///
    /// # Panics
    ///
    /// On a header that is not a valid header: the caller passes what a real
    /// request carried.
    pub async fn deliver(&self, route: &str, headers: &[(String, String)], body: &[u8]) -> Verdict {
        let mut request = Request::post(route);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::from(body.to_vec())).expect("a request"))
            .await
            .expect("a response");
        let status = response.status().as_u16();
        let sealed_answer = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/cose"));
        let _ = to_bytes(response.into_body(), usize::MAX).await;
        Verdict {
            status,
            sealed_answer,
        }
    }

    /// The plain envelope bodies the receiver's handler has seen, oldest first:
    /// only deliveries the layer accepted ever get here.
    ///
    /// # Panics
    ///
    /// If a handler panicked while holding the list.
    #[must_use]
    pub fn bodies(&self) -> Vec<String> {
        self.seen.lock().expect("seen bodies").clone()
    }
}
