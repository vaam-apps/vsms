//! The seam between this crate's hand-written [`token::TokenStore`] and
//! the generated client's request pipeline —
//! `cratestack::client_rust::RequestAuthorizer`. This is the trait issue
//! #171 pointed at as already shipping the extension point this SDK needed
//! (`cratestack-client-rust`'s own `auth.rs`); everything in this file is
//! the small adapter that plugs a token store into it.

use std::sync::Arc;

use cratestack::client_rust::{AuthorizationRequest, ClientError, RequestAuthorizer};

use crate::token::TokenStore;

/// Attaches `authorization: Bearer <token>` to every request the generated
/// client makes, sourcing the token from a [`TokenStore`]. Request
/// signing (`AuthorizationRequest::canonical_request`/`body`) is
/// deliberately unused — issue #171 is explicit that vsms dropped request
/// signing in favour of `private_key_jwt` (§4 of the design doc), and
/// those fields exist on `AuthorizationRequest` only because
/// `RequestAuthorizer` is a general-purpose trait other cratestack
/// deployments use for HMAC-style signing.
pub struct GatewayAuthorizer {
    token_store: Arc<dyn TokenStore>,
}

impl GatewayAuthorizer {
    pub fn new(token_store: Arc<dyn TokenStore>) -> Self {
        Self { token_store }
    }
}

#[async_trait::async_trait]
impl RequestAuthorizer for GatewayAuthorizer {
    async fn authorize(
        &self,
        _request: &AuthorizationRequest,
    ) -> Result<Vec<(String, String)>, ClientError> {
        let token = self
            .token_store
            .get_token()
            .await
            .map_err(|error| ClientError::BadInput(error.to_string()))?;
        Ok(vec![(
            "authorization".to_owned(),
            format!("Bearer {token}"),
        )])
    }
}
