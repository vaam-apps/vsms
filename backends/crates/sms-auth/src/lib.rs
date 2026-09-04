#![doc = include_str!("lib.md")]

pub mod login;
pub mod op;

use std::sync::Arc;

use async_trait::async_trait;
// authkestra-engine 0.8.0 moved `ClientStore`/`ClientAssertionStore` (and
// the error type both return) out of `authkestra-op` and into
// `authkestra-engine::store` — `authkestra_op` (which itself now depends
// on `authkestra_engine`, not the other way around) re-exports
// `ClientStore`/`ClientRegistration`/`GrantType`/`TokenEndpointAuthMethod`
// as a compatibility shim (`authkestra-op-0.8.0/src/client.rs` is a bare
// `pub use authkestra_engine::...` — verified against the vendored
// source, not assumed), but does **not** re-export `ClientAssertionStore`
// itself, only the two in-memory implementations of it
// (`MemoryClientAssertionStore`/`NoClientAssertionStore`). So that one
// import moves to its real home; the other three keep working unchanged
// through the compatibility re-export. See AGENTS.md's authkestra-0.8
// section, item A3.
use authkestra_engine::store::StoreError;
use authkestra_engine::store::traits::ClientAssertionStore;
use authkestra_op::{ClientRegistration, ClientStore, GrantType, TokenEndpointAuthMethod};
use chrono::{DateTime, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sms_api::errors::UNIQUE_VIOLATION;
use sms_api::schema::{self, ClientAuthMethod, Cratestack, oauth_client};
use sms_core::unpack;
use thiserror::Error;

/// Log the database-level detail and return the opaque error `authkestra`
/// expects.
///
/// Returns `authkestra_engine::store::StoreError`, not `authkestra_op::OpError`
/// — as of authkestra-engine 0.8.0, `ClientStore`/`ClientAssertionStore`
/// (the two traits this function backs) are defined against `StoreError`;
/// `authkestra_op::error::OpError`'s own `From<StoreError>` impl is what
/// collapses an opaque `StoreError::Internal(..)` into `OpError::Storage`
/// at the one call site inside the OP that actually needs an `OpError`
/// (`authkestra_op::handlers::authorize::handle_authorize`). That impl's
/// own doc comment is where "storage backends should not leak
/// implementation details (e.g. SQL errors) into OAuth error responses"
/// lives now — the reasoning this function's own doc used to cite
/// directly from `OpError::Storage` before that type moved out of the
/// return path here. Keeping detail out of the **response** was always
/// the point, not out of the **logs** — collapsing every `CratestackError`
/// into an opaque store error silently would make a policy denial (a
/// `sys` context that somehow lost the `system` role) indistinguishable
/// from a genuine outage in the one place a human could tell them apart.
fn log_and_opaque(context: &'static str, error: &CratestackError) -> StoreError {
    tracing::error!(context, error = %error, "sms-auth delegate call failed");
    StoreError::Internal("sms-auth delegate call failed".to_owned())
}

/// `OauthClient` as read from the database cannot become a valid
/// `ClientRegistration`.
#[derive(Debug, Error)]
enum RegistrationError {
    /// `jwks` is set but is not valid JSON.
    ///
    /// Should not be reachable past `INSERT` — `jwks::jsonb` in
    /// `oauth_clients_auth_method_jwks_check` (§2.10) already rejects
    /// anything the cast fails on — but `find_client` reads whatever is in
    /// the column, not what the constraint most recently allowed. A parse
    /// failure here means the constraint and this parser have drifted, and
    /// that is a defect in this crate to notice, not a client to reject.
    #[error("client {client_id} has malformed jwks: {source}")]
    MalformedJwks {
        client_id: String,
        #[source]
        source: serde_json::Error,
    },
}

/// Project a database row into what `authkestra-op` needs to authenticate a
/// client.
///
/// A free function, not inlined into [`SmsClientStore::find_client`], so it
/// can be exercised without a database — everything here is pure.
///
/// `#[allow(deprecated)]`: `ClientRegistration::require_pkce` is deprecated
/// as of authkestra-engine 0.8.0 — PKCE is now mandatory, unconditionally,
/// for every client on the authorization-code grant (OAuth 2.1 §4.1,
/// authkestra#273), so neither `handlers::authorize` nor `handlers::token`
/// reads this field any more (§4.3/#194's own login flow already sent PKCE
/// on every request, so nothing here changes behaviourally). The field
/// still has to be populated: `ClientRegistration` derives no `Default`, so
/// there is no `..Default::default()` seam to omit it through. `schema.cstack`'s
/// own `OauthClient.requirePkce` column is now dead weight this store reads
/// and immediately discards — worth a schema decision (drop the column) at
/// some point, but not one this dependency bump makes unilaterally; see
/// AGENTS.md's authkestra-0.8 section, item A5.
#[allow(deprecated)]
fn to_registration(row: schema::OauthClient) -> Result<ClientRegistration, RegistrationError> {
    let jwks = row
        .jwks
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|source| RegistrationError::MalformedJwks {
            client_id: row.clientId.clone(),
            source,
        })?;

    Ok(ClientRegistration {
        client_id: row.clientId,
        // No column can hold one. private_key_jwt means there is no shared
        // secret anywhere in this system — see OauthClient in schema.cstack
        // and §4.2. This is not a fallback for a missing value; it is the
        // only value that could ever be correct here.
        client_secret_hash: None,
        redirect_uris: unpack(&row.redirectUris)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        // Built in Rust from a delimited column, matching authkestra's own
        // wire strings exactly rather than round-tripping through its serde
        // impl. Kept even though the serde bug that originally motivated
        // this is fixed in 0.3.2 (#6) — this column is one `unpack` away
        // from a `Vec<GrantType>` either way, and matching the wire strings
        // by hand means a typo here is a missing grant, not a silent
        // `Custom` variant nothing matches against.
        grant_types: unpack(&row.grantTypes)
            .into_iter()
            .map(|g| match g {
                "authorization_code" => GrantType::AuthorizationCode,
                "refresh_token" => GrantType::RefreshToken,
                "client_credentials" => GrantType::ClientCredentials,
                "urn:ietf:params:oauth:grant-type:device_code" => GrantType::DeviceCode,
                "urn:ietf:params:oauth:grant-type:token-exchange" => GrantType::TokenExchange,
                other => GrantType::Custom(other.to_owned()),
            })
            .collect(),
        scopes: unpack(&row.scopes).into_iter().map(str::to_owned).collect(),
        require_pkce: row.requirePkce,
        // TokenRequest.audience and allowed_audiences apply only to token
        // exchange, which is unreachable on the client_credentials path this
        // system uses (§4.2) — token exchange needs claims.identity, and
        // client-credentials tokens always have identity: None. Kept
        // deliberately empty rather than parsed from anywhere, because
        // there is nowhere to parse it *from*: OauthClient has no
        // allowed-audiences column, and grantTypes has no CHECK stopping a
        // future row from listing "token_exchange" anyway (§2.10 enforces
        // none of its values).
        //
        // If that ever happens before this comment is revisited: verified
        // against authkestra_op::handlers::token that an *explicit*
        // TokenRequest.audience fails closed here — `allowed_audiences`
        // empty means `.contains(&requested_aud)` is always false, so the
        // exchange is refused with `invalid_target`. But an *omitted*
        // audience skips that check entirely and defaults to
        // `config.issuer.clone()`, succeeding regardless of
        // `allowed_audiences` — narrower than "accepts any audience," but
        // still "exchanges to the gateway's own issuer with no
        // per-resource scoping," which is not a decision this crate should
        // make silently. A real token_exchange client needs a schema
        // column here, not this empty vec.
        allowed_audiences: vec![],
        // Always Some: tokenEndpointAuthMethod is NOT NULL with no @default
        // (§2.2) precisely so this can never be the None that authkestra
        // reads as "predates the field" and answers by accepting a secret
        // from either transport while refusing every assertion.
        token_endpoint_auth_method: Some(match row.tokenEndpointAuthMethod {
            ClientAuthMethod::private_key_jwt => TokenEndpointAuthMethod::PrivateKeyJwt,
            ClientAuthMethod::none => TokenEndpointAuthMethod::NoAuth,
        }),
        jwks,
    })
}

/// Reads `OauthClient` for `authkestra_op::handlers::token`.
///
/// Per R1, this is a `CrateStack` delegate read, not raw `sqlx` — see the
/// worked example in §4.2 of the design doc, which this mirrors.
///
/// `#[derive(Clone)]`, required since authkestra-engine 0.8.0:
/// `authkestra_op::CloneableOpStore` (the trait `authkestra-axum`'s
/// `axum_token_handler` now requires — see `sms_auth::op`'s own doc, and
/// AGENTS.md item A2) is blanket-implemented for any `OpStore + Clone`,
/// and `CompositeOpStore<C, ...>` only derives `Clone` when every one of
/// its store type parameters does. This `Clone` shares state correctly —
/// see `CloneableOpStore`'s own doc on why that matters — because both
/// fields are themselves cheap, state-sharing handles: `Arc<Cratestack>`
/// clones the pool handle, not the pool, and `CratestackContext` was
/// already `Clone` before this (see `SmsClientStore::new`'s caller,
/// `machine_only_store`, which already called `.clone()` on both before
/// this derive existed).
#[derive(Clone)]
pub struct SmsClientStore {
    db: Arc<Cratestack>,
    sys: CratestackContext,
}

impl SmsClientStore {
    /// `sys` must be a `system`-role context — the only one `OauthClient`'s
    /// policy admits (`@@allow("read", hasRole('system'))`).
    #[must_use]
    pub fn new(db: Arc<Cratestack>, sys: CratestackContext) -> Self {
        Self { db, sys }
    }
}

#[async_trait]
impl ClientStore for SmsClientStore {
    // `&mut self`, not `&self`: authkestra-engine 0.8.0's `ClientStore`
    // trait (`authkestra_op::ClientStore` is now a re-export of it) takes
    // `&mut self` on every method — a real signature change this impl
    // must match exactly, not a mechanical relaxation. Nothing in this
    // body actually mutates `self`; the mutability exists so a stateful
    // backend (a connection with an in-flight transaction, a
    // non-thread-safe cache) *can*, not so every implementor must. See
    // AGENTS.md item A1.
    async fn find_client(
        &mut self,
        client_id: &str,
    ) -> Result<Option<ClientRegistration>, StoreError> {
        let found = self
            .db
            .oauth_client()
            .find_many()
            .where_expr(
                FilterExpr::from(oauth_client::clientId().eq(client_id))
                    .and(oauth_client::active().is_true()),
            )
            .limit(1)
            .run(&self.sys)
            .await
            .map_err(|e| log_and_opaque("SmsClientStore::find_client", &e))?;

        found
            .into_iter()
            .next()
            .map(to_registration)
            .transpose()
            .map_err(|e| {
                tracing::error!(error = %e, "sms-auth: stored client failed to parse");
                StoreError::Internal("sms-auth: stored client failed to parse".to_owned())
            })
    }
}

/// Records spent `private_key_jwt` assertion `jti`s in `ClientAssertion`, so
/// a captured assertion is single-use (RFC 7523 §3 point 7).
///
/// `record_jti` must be atomic — see `authkestra_op::client_assertion`'s
/// module docs — and `create` + catching `23505` on the `@unique` index on
/// `jti` is exactly that, and the only option: `upsert` does not exist when
/// the `@id` carries a default (§2.0), and a read-then-write across two
/// statements is the TOCTOU race the atomicity requirement exists to
/// prevent.
///
/// Deliberately write-only: this store never reads `client_assertion`, and
/// reaping expired rows is a scheduled job's concern (§2.10's
/// `client_assertions_expiry_idx` exists for that job's query), not
/// something a `record_jti` call should ever do inline.
///
/// `#[derive(Clone)]` for the identical reason [`SmsClientStore`] carries
/// one now — see that type's own doc.
#[derive(Clone)]
pub struct SmsClientAssertionStore {
    db: Arc<Cratestack>,
    sys: CratestackContext,
}

impl SmsClientAssertionStore {
    /// `sys` must be a `system`-role context, matching `ClientAssertion`'s
    /// policy.
    #[must_use]
    pub fn new(db: Arc<Cratestack>, sys: CratestackContext) -> Self {
        Self { db, sys }
    }
}

#[async_trait]
impl ClientAssertionStore for SmsClientAssertionStore {
    // `&mut self` and `StoreError`, not `&self`/`OpError` — same move as
    // `ClientStore` above (AGENTS.md item A3): `ClientAssertionStore`
    // moved to `authkestra_engine::store::traits` in 0.8.0, taking
    // `&mut self` and `StoreError` with it.
    async fn record_jti(
        &mut self,
        jti: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        match self
            .db
            .client_assertion()
            .create(schema::CreateClientAssertionInput {
                jti: jti.to_owned(),
                expiresAt: expires_at,
            })
            .run(&self.sys)
            .await
        {
            Ok(_) => Ok(true),
            // A duplicate jti is not a fault, it is the answer: this
            // assertion has been presented before and must be refused. Every
            // other database error is a real fault and stays opaque.
            //
            // This arm was unreachable through cratestack-sqlx =0.5.2,
            // which discarded SQLSTATE on every write (vsms#87); fixed in
            // 0.6.0. Written against the documented API throughout, so the
            // fix needed no change here — only the pin moved.
            Err(e) if e.db_sqlstate() == Some(UNIQUE_VIOLATION) => Ok(false),
            Err(e) => Err(log_and_opaque("SmsClientAssertionStore::record_jti", &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RegistrationError, to_registration};
    use authkestra_op::{GrantType, TokenEndpointAuthMethod};
    use sms_api::schema::{ClientAuthMethod, CreateOauthClientInput, OauthClient};

    /// Builds a plausible `OauthClient` row, with every field a test can
    /// override — mirroring `create_inputs.rs`'s exhaustive-construction
    /// style so a schema change that adds a field is a compile error here
    /// too.
    fn row(over: impl FnOnce(CreateOauthClientInput) -> CreateOauthClientInput) -> OauthClient {
        let input = over(CreateOauthClientInput {
            clientId: "otp-svc-v1".to_owned(),
            appClientId: None,
            tokenEndpointAuthMethod: ClientAuthMethod::private_key_jwt,
            jwks: Some(r#"{"keys":[{"kty":"RSA","kid":"k1","n":"x","e":"AQAB"}]}"#.to_owned()),
            grantTypes: " client_credentials ".to_owned(),
            scopes: " sms:send sms:read ".to_owned(),
            redirectUris: " ".to_owned(),
            requirePkce: false,
        });

        OauthClient {
            id: "c00000000000000000000001".to_owned(),
            createdAt: chrono::Utc::now(),
            updatedAt: chrono::Utc::now(),
            clientId: input.clientId,
            appClientId: input.appClientId,
            tokenEndpointAuthMethod: input.tokenEndpointAuthMethod,
            jwks: input.jwks,
            grantTypes: input.grantTypes,
            scopes: input.scopes,
            redirectUris: input.redirectUris,
            requirePkce: input.requirePkce,
            active: true,
        }
    }

    #[test]
    // `require_pkce` is deprecated (authkestra-engine 0.8.0, PKCE now
    // mandatory unconditionally — see `to_registration`'s own doc, item
    // A5) but this test still asserts the mapping round-trips correctly:
    // the column and the field both still exist, `to_registration` still
    // maps one onto the other, and that mapping is still worth a passing
    // test even though nothing downstream reads the result any more.
    #[allow(deprecated)]
    fn maps_a_private_key_jwt_client_end_to_end() {
        let reg = to_registration(row(|i| i)).expect("valid row parses");

        assert_eq!(reg.client_id, "otp-svc-v1");
        assert_eq!(
            reg.client_secret_hash, None,
            "no column can hold a shared secret"
        );
        assert_eq!(
            reg.token_endpoint_auth_method,
            Some(TokenEndpointAuthMethod::PrivateKeyJwt)
        );
        assert!(reg.jwks.is_some());
        assert_eq!(reg.grant_types, vec![GrantType::ClientCredentials]);
        assert_eq!(reg.scopes, vec!["sms:send", "sms:read"]);
        assert!(!reg.require_pkce);
        assert!(reg.allowed_audiences.is_empty());
    }

    #[test]
    // Same reasoning as the previous test's own `#[allow(deprecated)]`.
    #[allow(deprecated)]
    fn maps_a_public_client_with_no_jwks() {
        let reg = to_registration(row(|i| CreateOauthClientInput {
            tokenEndpointAuthMethod: ClientAuthMethod::none,
            jwks: None,
            grantTypes: " authorization_code refresh_token ".to_owned(),
            redirectUris: " https://admin.example/cb ".to_owned(),
            requirePkce: true,
            ..i
        }))
        .expect("valid row parses");

        assert_eq!(
            reg.token_endpoint_auth_method,
            Some(TokenEndpointAuthMethod::NoAuth)
        );
        assert_eq!(reg.jwks, None);
        assert_eq!(
            reg.grant_types,
            vec![GrantType::AuthorizationCode, GrantType::RefreshToken]
        );
        assert_eq!(reg.redirect_uris, vec!["https://admin.example/cb"]);
        assert!(reg.require_pkce);
    }

    #[test]
    fn an_unrecognised_grant_string_becomes_custom_rather_than_vanishing() {
        // Defense in depth: the schema names no CHECK over grant_types, so a
        // typo or a future grant type must not be silently dropped from the
        // registration — Custom keeps it visible to authkestra, which then
        // refuses it for a legible reason (unauthorized_client) rather than
        // this store refusing it for an invisible one.
        let reg = to_registration(row(|i| CreateOauthClientInput {
            grantTypes: " some_future_grant ".to_owned(),
            ..i
        }))
        .expect("valid row parses");

        assert_eq!(
            reg.grant_types,
            vec![GrantType::Custom("some_future_grant".to_owned())]
        );
    }

    #[test]
    fn device_code_and_token_exchange_urns_round_trip() {
        let reg = to_registration(row(|i| CreateOauthClientInput {
            grantTypes: " urn:ietf:params:oauth:grant-type:device_code \
                          urn:ietf:params:oauth:grant-type:token-exchange "
                .to_owned(),
            ..i
        }))
        .expect("valid row parses");

        assert_eq!(
            reg.grant_types,
            vec![GrantType::DeviceCode, GrantType::TokenExchange]
        );
    }

    #[test]
    fn an_empty_scopes_column_maps_to_no_scopes_not_one_empty_scope() {
        // The sentinel encoding's empty form is a single space (sms-core),
        // not the empty string. `unpack(" ")` must yield zero scopes, or
        // every client would silently register for a scope literally named
        // "".
        let reg = to_registration(row(|i| CreateOauthClientInput {
            scopes: " ".to_owned(),
            ..i
        }))
        .expect("valid row parses");

        assert!(reg.scopes.is_empty());
    }

    #[test]
    fn malformed_jwks_is_a_parse_error_not_a_panic() {
        // Should not be reachable past the §2.10 CHECK, which casts to
        // jsonb — but find_client must not trust that nothing else can ever
        // write this column, so it fails loudly rather than unwrapping.
        let err = to_registration(row(|i| CreateOauthClientInput {
            jwks: Some("not json".to_owned()),
            ..i
        }))
        .unwrap_err();

        assert!(matches!(err, RegistrationError::MalformedJwks { .. }));
    }
}
