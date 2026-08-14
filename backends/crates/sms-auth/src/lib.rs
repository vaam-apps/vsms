//! `authkestra_op::OpStore` implementations backed by `CrateStack` delegates.
//!
//! Two pieces, matching the two things `authkestra-op` needs to know that only
//! this database has:
//!
//! - [`SmsClientStore`] — `ClientStore::find_client`, reading `OauthClient`.
//! - [`SmsClientAssertionStore`] — `ClientAssertionStore::record_jti`,
//!   reading and writing `ClientAssertion`.
//!
//! Both exist because R1 says all data access goes through `CrateStack`
//! delegates, never raw `sqlx` — `authkestra-op` ships a `sqlx_store.rs` of
//! its own, but adopting it would mean bypassing row-level policy, `@@audit`
//! and `@@emit`, which is exactly what R1 exists to prevent.
//!
//! Neither type mounts a router or owns a connection pool — `sms-auth` links
//! `sms-api` for the expanded schema and nothing else. Wiring these into
//! `authkestra_op::CompositeOpStore` and the OP router itself is #20; wiring
//! their output into an `AuthProvider` is #21.
//!
//! # Why `record_jti` reads `db_sqlstate()` rather than pre-checking
//!
//! [`SmsClientAssertionStore::record_jti`]'s replay check is `create` plus
//! catching `23505`, not a `SELECT` followed by an `INSERT` — the pre-check
//! form races, and `upsert` does not exist when the `@id` carries a default
//! (§2.0). That makes it dependent on the driver's SQLSTATE surviving the
//! framework's sqlx→`CoolError` conversion.
//!
//! For the whole of `cratestack-sqlx` `=0.5.0`–`=0.5.2` it did not: every
//! generated write mapped through `CoolError::Database(error.to_string())`,
//! discarding SQLSTATE and constraint before application code saw them, so
//! `db_sqlstate()` was `None` on every database-rejected write. Filed as
//! [cratestack/cratestack#267](https://github.com/cratestack/cratestack/issues/267),
//! tracked here as [vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87),
//! **fixed in `cratestack-sqlx` 0.6.0** — all twelve write paths now route
//! through `cool_error_from_sqlx`.
//!
//! `tests/live_postgres.rs`'s `record_jti_is_true_once_and_false_on_replay`
//! was written to assert the correct behaviour while that bug was live, and
//! left failing rather than weakened, so it would go green the moment the
//! pin moved. It does. Keep it, and the sibling assertions in
//! `sms-api`'s `tests/errors_live_postgres.rs`: they are the only coverage
//! that can see this class of regression, since a hand-constructed
//! `CoolError::DatabaseTyped` never exercises the conversion, and
//! `cargo build` / `cratestack check` stay green through it either way.

pub mod login;
pub mod op;

use std::sync::Arc;

use async_trait::async_trait;
use authkestra_op::{
    ClientAssertionStore, ClientRegistration, ClientStore, GrantType, OpError,
    TokenEndpointAuthMethod,
};
use chrono::{DateTime, Utc};
use cratestack::{CoolContext, CoolError, FilterExpr};
use sms_api::errors::UNIQUE_VIOLATION;
use sms_api::schema::{self, oauth_client, ClientAuthMethod, Cratestack};
use sms_core::unpack;
use thiserror::Error;

/// Log the database-level detail and return the opaque error `authkestra-op`
/// expects.
///
/// `OpError::Storage`'s own doc comment says why it carries no detail:
/// *"storage backends should not leak implementation details (e.g. SQL
/// errors) into OAuth error responses."* That is a reason to keep the detail
/// out of the **response**, not out of the **logs** — collapsing every
/// `CoolError` into `Storage` silently would make a policy denial (a `sys`
/// context that somehow lost the `system` role) indistinguishable from a
/// genuine outage in the one place a human could tell them apart.
fn log_and_opaque(context: &'static str, error: &CoolError) -> OpError {
    tracing::error!(context, error = %error, "sms-auth delegate call failed");
    OpError::Storage
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
pub struct SmsClientStore {
    db: Arc<Cratestack>,
    sys: CoolContext,
}

impl SmsClientStore {
    /// `sys` must be a `system`-role context — the only one `OauthClient`'s
    /// policy admits (`@@allow("read", hasRole('system'))`).
    #[must_use]
    pub fn new(db: Arc<Cratestack>, sys: CoolContext) -> Self {
        Self { db, sys }
    }
}

#[async_trait]
impl ClientStore for SmsClientStore {
    async fn find_client(&self, client_id: &str) -> Result<Option<ClientRegistration>, OpError> {
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
                OpError::Storage
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
pub struct SmsClientAssertionStore {
    db: Arc<Cratestack>,
    sys: CoolContext,
}

impl SmsClientAssertionStore {
    /// `sys` must be a `system`-role context, matching `ClientAssertion`'s
    /// policy.
    #[must_use]
    pub fn new(db: Arc<Cratestack>, sys: CoolContext) -> Self {
        Self { db, sys }
    }
}

#[async_trait]
impl ClientAssertionStore for SmsClientAssertionStore {
    async fn record_jti(&self, jti: &str, expires_at: DateTime<Utc>) -> Result<bool, OpError> {
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
    use super::{to_registration, RegistrationError};
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
