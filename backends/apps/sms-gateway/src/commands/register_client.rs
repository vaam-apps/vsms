//! `Command::RegisterClient` - see that variant's own doc comment in
//! `main.rs` for what this does and why it exists (signed transport, VS-2).

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::client_keys::{parse_public_key, register};
use sms_api::schema::procedures::register_app_client_key;
use sms_api::schema::{
    Cratestack, CreateAppClientInput, RegisterClientKeyInput, app as app_filter,
    app_client as app_client_filter,
};
use sms_api::{Principal, PrincipalKind, system_context};
use sms_core::pack;

/// `Command::RegisterClient`'s flags. See `Command::RegisterClient`'s own doc
/// comment in `main.rs` - the enum variant carries the "why", this struct only
/// carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct RegisterClientArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// The `App.slug` the client acts for. The App must already exist and be
    /// active (`sms-gateway create-app`), exactly like `provision-client
    /// --app-slug`; this command never creates an App, whose quota and
    /// allowlist are a business decision.
    #[arg(long = "app")]
    pub(crate) app_slug: String,

    /// The client's Ed25519 public key: the raw 32 bytes, base64url without
    /// padding. The private half stays with whoever generated it and never
    /// reaches vsms. Not secret, so it is a plain flag.
    #[arg(long)]
    pub(crate) public_key: String,

    /// A human-readable label for the key (1-64 characters), e.g.
    /// `"console composer"`.
    #[arg(long)]
    pub(crate) label: Option<String>,

    /// Scopes for the app's signing client, e.g. `--scope sms:send`. Needed
    /// only the first time, when the client is created; an existing client
    /// keeps the scopes it has and these are ignored.
    #[arg(long = "scope")]
    pub(crate) scopes: Vec<String>,

    /// Which of `registerAppClientKey`'s two admitted roles to run the call
    /// under. Same choice and default as `provision-client --role`.
    #[arg(long, default_value = "owner")]
    pub(crate) role: String,
}

/// The `AppClient.clientId` the signed-transport key of `slug` hangs off. A
/// pure function of the slug, so a second run (or a second key for the same
/// app) finds the client the first one made instead of minting another, and
/// `AppClient.clientId`'s unique index makes the find-or-create atomic.
/// `provision-client` instead mints a random `appc_<uuid>` per run, which is
/// why running it twice leaves two clients.
fn client_id_for(slug: &str) -> String {
    format!("appc_{slug}")
}

/// `Command::RegisterClient`'s body. Idempotent: the key is identified by its
/// RFC 9679 thumbprint, so the same key registered twice leaves one row and
/// exits 0; the client by [`client_id_for`].
pub(crate) async fn register_client_command(args: RegisterClientArgs) -> Result<()> {
    let RegisterClientArgs {
        database_url,
        app_slug,
        public_key,
        label,
        scopes,
        role,
    } = args;

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" - registerAppClientKey's own @allow admits \
             nothing else, got {role:?}"
        );
    }
    if app_slug.trim().is_empty() {
        bail!("--app must not be empty - pass the App.slug (see `sms-gateway create-app`)");
    }
    // Before the database, so a typo'd key fails with nothing touched.
    let parsed = parse_public_key(&public_key).context("--public-key")?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:register-client");
    let ctx = Principal {
        sub: format!("sms-gateway:register-client:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let app = db
        .app()
        .find_many()
        .where_expr(FilterExpr::from(app_filter::slug().eq(app_slug.clone())))
        .limit(1)
        .run(&ctx)
        .await
        .context("resolving --app to an App")?
        .into_iter()
        .next()
        .with_context(|| format!("no App with slug {app_slug:?} exists"))?;
    if !app.active {
        bail!("App {app_slug:?} is not active");
    }

    let client_id = client_id_for(&app_slug);
    let (app_client_id, created) =
        create_or_find_client(&db, &sys, &app.id, &client_id, &scopes).await?;

    let input = RegisterClientKeyInput {
        appClientId: app_client_id,
        publicKey: public_key,
        label,
    };
    let call = register_app_client_key::Args { args: input };
    // The same entry point the HTTP route uses, so `@allow` is the real one
    // and this command cannot register a key a console session could not.
    // The witness `invoke_with_db` hands in proves that; `register` is the
    // procedure body, shared with `Procedures`.
    let key = register_app_client_key::invoke_with_db(&db, &call, &ctx, |db, _authorized| {
        let (sys, input) = (sys.clone(), call.args.clone());
        async move { register(&db, &sys, input).await }
    })
    .await
    .context("registering the client key")?;

    println!(
        "app client: {client_id} ({})",
        if created { "created" } else { "existing" }
    );
    println!("key id: {}", key.id);
    println!("thumbprint: {}", key.thumbprint);
    debug_assert_eq!(key.thumbprint, parsed.thumbprint);
    Ok(())
}

/// Finds the app's signing client, or creates it. `create` + catching the
/// `23505` on `AppClient.clientId`'s unique index, the repo's dedupe idiom
/// (`upsert` does not exist when the `@id` has a default).
async fn create_or_find_client(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    app_id: &str,
    client_id: &str,
    scopes: &[String],
) -> Result<(String, bool)> {
    if let Some(existing) = find_client(db, sys, client_id).await? {
        return same_app(existing, app_id, client_id).map(|id| (id, false));
    }
    if scopes.is_empty() {
        bail!(
            "no signing client exists for this app yet - pass at least one --scope (e.g. \
             --scope sms:send) so it is created with something it may call"
        );
    }
    let scopes_packed = pack(scopes).map_err(|error| anyhow::anyhow!("--scope: {error}"))?;
    match db
        .app_client()
        .create(CreateAppClientInput {
            appId: app_id.to_owned(),
            clientId: client_id.to_owned(),
            label: "signed transport".to_owned(),
            scopes: scopes_packed,
            lastUsedAt: None,
            retiredAt: None,
        })
        .run(sys)
        .await
    {
        Ok(created) => Ok((created.id, true)),
        Err(error) if error.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {
            let existing = find_client(db, sys, client_id)
                .await?
                .context("AppClient reported as a duplicate on create but not found on lookup")?;
            same_app(existing, app_id, client_id).map(|id| (id, false))
        }
        Err(error) => Err(error).context("creating the app's signing client"),
    }
}

async fn find_client(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    client_id: &str,
) -> Result<Option<sms_api::schema::AppClient>> {
    Ok(db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            app_client_filter::clientId().eq(client_id.to_owned()),
        ))
        .limit(1)
        .run(sys)
        .await
        .context("looking up the app's signing client")?
        .into_iter()
        .next())
}

/// A client id is global, so one that exists but belongs to another App (a
/// slug that collides with an old `provision-client` id) is refused, never
/// silently reused.
fn same_app(existing: sms_api::schema::AppClient, app_id: &str, client_id: &str) -> Result<String> {
    if existing.appId != app_id {
        bail!("AppClient {client_id:?} already exists and belongs to a different App");
    }
    Ok(existing.id)
}

#[cfg(test)]
mod tests {
    use super::client_id_for;

    #[test]
    fn the_client_id_is_a_pure_function_of_the_slug() {
        assert_eq!(client_id_for("vaam"), "appc_vaam");
        assert_eq!(client_id_for("vaam"), client_id_for("vaam"));
        // `AppClient.clientId` is `@length(min: 8)`; `App.slug` is at least 3.
        assert!(client_id_for("abc").len() >= 8);
    }
}
