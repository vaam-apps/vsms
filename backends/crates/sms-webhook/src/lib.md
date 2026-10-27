Outbound webhooks as sealed requests (VS-8): the delivery contract, the route
rule and the sealer. Pure and schema-free; `backends/crates/sms-worker`'s
`hooks` role is the caller.

A delivery used to be an HMAC-SHA256 over a canonical string, keyed by a
per-endpoint shared secret (`WebhookEndpoint.secret`, `prevSecret`,
`X-Sms-Signature`). That is a shared secret, and this system has none: every
request to vaam, vpay or vsms is a `COSE_Sign1` envelope from a registered key
(owner's ruling, "keypairs everywhere"). So a delivery is now a **sealed
request**, the same envelope the gateway's own routes open, signed with vsms's
envelope signing key. The receiver pins vsms's public keys, the ones it already
pins to open vsms's answers (`sms-gateway server-keys`), and verifies with a
cratestack `EnvelopeLayer`. There is no per-endpoint secret, no rotation
overlap and no `prevSecret`.

```mermaid
sequenceDiagram
    participant W as sms-worker hooks
    participant S as Sealer (this crate)
    participant R as Receiver's EnvelopeLayer (Required)
    participant H as Receiver's handler
    W->>S: seal(audience, endpoint url, envelope JSON)
    S->>S: route = path of the url, digest = CONTRACT_SHA, fresh cti, iat = now
    S-->>W: COSE_Sign1 + Content-Type / Cratestack-* headers
    W->>R: POST route (the sealed bytes)
    R->>R: audience, method, route, digest, payload type, key, iat, nonce
    R->>H: the plain JSON envelope + VerifiedSigner
    H-->>R: 2xx
    R-->>W: 2xx, sealed under the receiver's own key
    Note over W: a 2xx is delivered; the body is not read
```

```mermaid
stateDiagram-v2
    [*] --> Sealed: Sealer::seal, per attempt
    Sealed --> Opened: every check passes at the receiver
    Sealed --> Refused401: wrong audience, route, digest, key, stale iat, replayed cti, tampered bytes
    Refused401 --> Resealed: the worker retries on its backoff
    Resealed --> Opened: a new cti and a new iat
    Opened --> [*]
```

# What a delivery binds

[`Sealer::seal`] calls cratestack's `ClientEnvelope::seal_call`. The seal binds,
and the receiver rebuilds from its own configuration:

| Bound value     | A delivery's value                                              | Where the receiver gets it                         |
| --------------- | --------------------------------------------------------------- | -------------------------------------------------- |
| audience        | `WebhookEndpoint.audience`, chosen when the endpoint is created | the name its `EnvelopeLayer` is built with         |
| method          | `POST` ([`METHOD`])                                             | the route it mounts                                |
| route           | the **path** of `WebhookEndpoint.url` ([`delivery_route`])      | the template axum matched (`/webhooks/vsms`)       |
| path parameters | none                                                            | the route has none                                 |
| query           | none (a URL with a query is refused)                            | none                                               |
| contract digest | [`CONTRACT_SHA`], from the document in [`CONTRACT_DOCUMENT`]    | its `AcceptedContracts` row for `POST <route>`     |
| payload type    | `application/json` ([`PAYLOAD_TYPE`]), and it asks for the same | `payload_media_types(["application/json"], [...])` |
| nonce, `iat`    | a fresh 16-byte `cti` and the worker's clock, **per attempt**   | the layer's nonce store and its 300 s skew         |

The `kid` in the message is the thumbprint prefix of vsms's public key; the
receiver's resolver maps it to the key it pinned.

The envelope JSON is `{id, eventId, type, occurredAt, data}`. `id` is
`WebhookAttempt.id`, the dedupe key §8.5 already names; `eventId` is
`WebhookAttempt.sourceEventId`, which used to travel only in the
`X-Sms-Event-Id` header and so was signed by the HMAC. Headers are not part of a
seal, so the id moved into the signed body. The `X-Sms-Event`, `X-Sms-Event-Id`,
`X-Sms-Timestamp` and `X-Sms-Signature` headers are gone: nothing outside the
sealed body is meant to be believed.

# What a receiver configures

Everything below is public; none of it is a secret.

1. **The pinned keys.** `current` and `next` from `sms-gateway server-keys`.
   Resolve a `kid` to either (`StaticVerifierResolver` with both
   `CoseVerifyKey::ed25519`). Pinning both makes a rotation of vsms's key a
   non-event.
2. **The audience.** Any non-empty name the receiver picks, set once on the
   `WebhookEndpoint` and in `EnvelopeLayer::builder(.., audience, ..)`. It must
   differ from the audience the receiver seals its *own* requests to vsms for
   (`vsms`); a name used in both directions gives up the reflection protection.
3. **The route.** One literal `POST` route, e.g. `/webhooks/vsms`, and the
   endpoint URL ends in that path. A tiny `BindingResolver` names it.
4. **The contract.** The row `("POST /webhooks/vsms", &[CONTRACT_SHA])` in the
   `AcceptedContracts` table, with [`CONTRACT_SHA_HEX`]
   `7c5a6ab84270ec64eeb668cf50a031ac436433a08467cefe142e338c93132b62`. Derive it
   as `SHA-256("vsms/webhook-delivery/v1\0" || CONTRACT_DOCUMENT)`.
5. **The mode.** `EnvelopeMode::Required`, JSON payloads only, a nonce store
   shared by every replica (the in-memory one is per process).

```text
struct WebhookRoutes;
impl BindingResolver for WebhookRoutes {
    fn resolve(&self, r: &RouteRequest<'_>) -> Resolution {
        match r.matched_path() {
            Some(path @ "/webhooks/vsms") if r.method() == Method::POST => {
                Resolution::Op(ResolvedRoute::new(path.to_owned(), Vec::new()))
            }
            _ => Resolution::Unresolved,
        }
    }
}
static ACCEPTED: AcceptedContracts = &[("POST /webhooks/vsms", &[CONTRACT_SHA])];
let layer = EnvelopeLayer::builder(envelope, "vaam", ACCEPTED)
    .binding_resolver(WebhookRoutes)
    .policy(EnvelopeMode::Required)
    .payload_media_types(["application/json"], ["application/json"])
    .build()?;
```

`tests/sealed_delivery.rs` is this snippet running: a real `EnvelopeLayer`
accepts a delivery and refuses another audience, another route, a tampered body,
a replay, an unpinned key, a stale `iat` and an undeclared contract.

# Retries and replay

The nonce and timestamp stop a captured delivery from being replayed; they do
not make a retry safe, and they must not be confused with dedupe. A retry is a
**new** sealed request ([`Sealer::seal`] runs again, with a fresh `cti` and
`iat`, so a retry after the receiver's 300 s skew still opens), carrying the
same `id` and `eventId`. Delivery is at-least-once: dedupe on `id`.

# The answer

A delivery is delivered when the receiver answers 2xx; 410 deactivates the
endpoint; anything else retries (`docs/architecture.md` §8.5, unchanged). If the
receiver is a cratestack layer its answer is a sealed message under the
receiver's own key. The worker does **not** open it and treats it as proof of
nothing: opening it would need the receiver's public keys pinned per endpoint,
which is one more credential store, one more console field and one more
rotation story for a property the protocol does not rest on. A 2xx is the
receiver's HTTP server saying it accepted the request, authenticated by TLS to
the endpoint's host, exactly as before. An unsigned 2xx is accepted on the same
terms, and never read.

# Public API

```text
METHOD, PAYLOAD_TYPE, CONTRACT_DOMAIN, CONTRACT_DOCUMENT, CONTRACT_SHA, CONTRACT_SHA_HEX
contract_sha() -> [u8; 32]
delivery_route(url: &str) -> Result<String, SealError>
Sealer::new(signer: Ed25519Signer) -> Result<Sealer, SealError>
Sealer::from_envelope(cose: CoseEnvelope) -> Result<Sealer, SealError>
Sealer::seal(&self, audience: &str, url: &str, body: &[u8]) -> Result<SealedDelivery, SealError>
```

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
