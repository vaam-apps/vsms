#![doc = include_str!("human_login.md")]

use serde::Deserialize;

use crate::model::Bucket;

/// The shortest and longest `client_id` `OauthClient.clientId` admits
/// (`@length(min: 8, max: 64)` in `schemas/vsms.cstack`).
pub const CLIENT_ID_LENGTH: std::ops::RangeInclusive<usize> = 8..=64;

/// `human_login:` - the `human_login` method's block: the one console client a
/// person's sign-in is issued to, and the limit in front of the sign-in.
///
/// Present means the sign-in is served (when `auth.methods` permits it and `op:`
/// is written, because the sign-in rides the `OpenID` Provider); absent means it
/// is not. Every field is required: the base file states the values.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct HumanLogin {
    /// The console, the one `OpenID` client a person signs in through.
    pub console_client: ConsoleClient,
    /// The bucket in front of `POST /login`, applied twice: once per email
    /// address and once per source address, both before any password is hashed.
    pub login_rate_limit: Bucket,
}

/// `human_login.console_client:` - the public client the console is registered
/// as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ConsoleClient {
    /// The `client_id` of the console's `OauthClient` row, 8 to 64 characters
    /// ([`CLIENT_ID_LENGTH`]). `POST /login` accepts this value and no other.
    pub client_id: String,
    /// The redirect URIs the console may be sent back to, each matched exactly
    /// by the provider. At least one. `seed-console-client` registers exactly
    /// this list, and `serve` refuses to start when the row differs.
    pub redirect_uris: Vec<String>,
}
