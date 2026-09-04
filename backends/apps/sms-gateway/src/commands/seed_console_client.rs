//! `Command::SeedConsoleClient` (#194) — see that variant's own doc
//! comment in `main.rs` for what this registers and why it's a public
//! (`token_endpoint_auth_method = none`) client.

use anyhow::{Context, Result};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{ClientAuthMethod, Cratestack, CreateOauthClientInput};

use sms_api::system_context;

/// `Command::SeedConsoleClient`'s flags. See `Command::SeedConsoleClient`'s
/// own doc comment in `main.rs` — the enum variant carries the "why",
/// this struct only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct SeedConsoleClientArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,

    /// Must match `sms-gateway serve --console-client-id` and
    /// `admin`'s own `SMS_CONSOLE_OIDC_CLIENT_ID` exactly —
    /// `GatewayAuth`'s human-token audience check
    /// (`sms_api::auth::GatewayAuth`'s own doc) refuses any other
    /// value outright.
    #[arg(long, env = "SMS_CONSOLE_OIDC_CLIENT_ID", default_value = sms_api::DEFAULT_CONSOLE_CLIENT_ID)]
    pub(crate) client_id: String,

    /// The exact, single `redirect_uri` this client is registered
    /// with — `authkestra_op::handlers::authorize::handle_authorize`
    /// requires an exact string match (RFC 6749 §3.1.2), not a prefix
    /// or origin match. Must equal `{ADMIN_BASE_URL}/api/auth/callback`
    /// from `admin`'s own `@vsms/env` schema.
    #[arg(long)]
    pub(crate) redirect_uri: String,
}

/// `Command::SeedConsoleClient`'s body (#194) — see that variant's own doc
/// comment for what this does and why. Idempotent, same
/// create-then-catch-23505 shape as `create_or_find_provider`
/// (`commands::seed_dispatch`). Takes [`SeedConsoleClientArgs`] directly
/// rather than the whole `Command`: `main`'s own dispatch already
/// extracted it from `Command::SeedConsoleClient` at the match site.
pub(crate) async fn seed_console_client_command(args: SeedConsoleClientArgs) -> Result<()> {
    let SeedConsoleClientArgs {
        database_url,
        client_id,
        redirect_uri,
    } = args;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:op");

    seed_console_client_core(&db, &sys, &client_id, &redirect_uri).await
}

/// The actual `sms-console` `OauthClient` seeding logic, shared by
/// [`seed_console_client_command`] and `bootstrap_command` — see
/// `seed_dispatch_core`'s (`commands::seed_dispatch`) own doc for why this
/// split exists.
pub(crate) async fn seed_console_client_core(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    client_id: &str,
    redirect_uri: &str,
) -> Result<()> {
    let input = CreateOauthClientInput {
        clientId: client_id.to_owned(),
        appClientId: None,
        tokenEndpointAuthMethod: ClientAuthMethod::none,
        jwks: None,
        grantTypes: " authorization_code refresh_token ".to_owned(),
        scopes: " openid profile ".to_owned(),
        redirectUris: format!(" {redirect_uri} "),
        requirePkce: true,
    };

    match db.oauth_client().create(input).run(sys).await {
        Ok(created) => {
            println!(
                "registered sms-console OauthClient {} (clientId={client_id:?})",
                created.id
            );
        }
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {
            println!(
                "sms-console OauthClient with clientId={client_id:?} already exists — nothing \
                 to do (redirect_uri is not updated on an existing row by this command; re-run \
                 by hand against the database if it needs to change)"
            );
        }
        Err(e) => return Err(e).context("seeding the sms-console OauthClient row"),
    }
    Ok(())
}
