#![allow(clippy::ptr_arg)]
#![allow(missing_docs)]

use async_trait::async_trait;
use authkestra_op::{ClientRegistration, ClientStore, GrantType, OpError};
use cratestack::{CoolContext, FilterExpr};
use std::sync::Arc;

cratestack::include_server_schema!("../../schema/schema.cstack", db = Postgres);
pub use crate::cratestack_schema as schema;

use schema::{oauth_client, Cratestack};

pub struct SmsClientStore {
    db: Arc<Cratestack>,
    sys: CoolContext,
}

impl SmsClientStore {
    pub fn new(db: Arc<Cratestack>, sys: CoolContext) -> Self {
        Self { db, sys }
    }
}

fn unpack(s: &str) -> Vec<String> {
    s.split_whitespace().map(|s| s.to_owned()).collect()
}

fn to_op_error(_e: cratestack::CoolError) -> OpError {
    OpError::Storage
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
            .map_err(to_op_error)?;

        Ok(found.into_iter().next().map(|c| ClientRegistration {
            client_id: c.clientId,
            // NOT NULL in the schema. A None hash disables authentication
            // entirely — see sharp edge 1 below.
            client_secret_hash: Some(c.secretHash),
            redirect_uris: unpack(&c.redirectUris),
            // Built in Rust from a delimited column. serde never touches
            // GrantType, so the untagged bug cannot bite.
            grant_types: unpack(&c.grantTypes)
                .iter()
                .map(|g| match g.as_str() {
                    "client_credentials" => GrantType::ClientCredentials,
                    "authorization_code" => GrantType::AuthorizationCode,
                    "refresh_token" => GrantType::RefreshToken,
                    other => GrantType::Custom(other.to_owned()),
                })
                .collect(),
            scopes: unpack(&c.scopes),
            require_pkce: c.requirePkce,
            allowed_audiences: vec![],
            token_endpoint_auth_method: None,
            jwks: None,
        }))
    }
}
