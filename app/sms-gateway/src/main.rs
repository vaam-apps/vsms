//! The SMS gateway API server.

mod dlr;
mod op;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::FilterExpr;
use sms_api::schema::procedures::{provision_app_client, ProcedureRegistry};
use sms_api::schema::{provider as provider_filter, Cratestack, ProvisionClientInput};
use sms_api::{GatewayAuth, Principal, PrincipalKind, Procedures};
use sms_provider::SmsProvider;
use tracing::info;

/// Command-line surface.
#[derive(Debug, Parser)]
#[command(name = "sms-gateway", version, about = "A2P SMS gateway for Cameroon")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bind the HTTP API.
    Serve {
        /// Address to listen on. Loopback by default: TLS terminates at a Caddy
        /// or nginx edge, and this process should never face the internet.
        #[arg(long, env = "SMS_LISTEN_ADDR", default_value = "127.0.0.1:8080")]
        listen: String,

        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// Maximum pooled connections.
        #[arg(long, env = "SMS_DB_MAX_CONNECTIONS", default_value_t = 10)]
        max_connections: u32,

        /// The OP's own identity — every token this OP mints carries this
        /// as `iss`, and `GatewayAuth` validates incoming tokens against
        /// exactly this value. Never `listen` (a bind address, not an
        /// identity) — must be the externally reachable `https://` origin
        /// this OP is actually served at.
        #[arg(long, env = "SMS_OIDC_ISSUER")]
        issuer: String,

        /// `OAuth2` `client_credentials` client id for Orange Cameroon's
        /// SMS API — required unconditionally, unlike `sms-worker`'s own
        /// copy of this flag (optional there, only needed when `dispatch`
        /// is selected): this binary always serves the DLR route (#34),
        /// which always needs a provider to parse against.
        #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
        orange_client_id: String,

        /// Paired with `orange_client_id`. Never logged.
        #[arg(long, env = "ORANGE_CM_CLIENT_SECRET")]
        orange_client_secret: String,

        /// E.164 without the `tel:` scheme.
        #[arg(long, env = "ORANGE_CM_SENDER_NUMBER")]
        orange_sender_number: String,

        /// Overridable so a real Orange sandbox (not just this crate's own
        /// `wiremock`-backed tests) can be pointed at without a code change.
        #[arg(
            long,
            env = "ORANGE_CM_BASE_URL",
            default_value = "https://api.orange.com"
        )]
        orange_base_url: String,

        /// #134: the server-held pepper behind `Message.msisdnHash`/
        /// `Message.bodyHash` — real secret material, config only, never
        /// the database, a migration, or a log line (see `sms_api::pepper`'s
        /// module doc for the scheme and the rotation consequence).
        /// Required unconditionally and validated (minimum length) before
        /// this process does anything else, so a missing or trivially weak
        /// pepper fails loudly at startup — never silently at the first
        /// `sendMessage` call. Never logged: `HashPepper`'s own `Debug`
        /// impl redacts it even if this struct were ever printed.
        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: String,
    },
    /// Print the generated route table and exit. Needs no database.
    Routes,
    /// Generate a new RSA signing key, activate it, and keep the previous
    /// one publishing in JWKS for `sms_auth::op::ROTATION_OVERLAP` — an
    /// operator action, not a generated-CRUD route (`OauthSigningKey`'s own
    /// schema comment: this is the key that signs every token the OP
    /// issues, and it must never be reachable except as `hasRole('system')`
    /// already restricts it to).
    RotateSigningKey {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
    },
    /// Mint a real, HTTP-usable `private_key_jwt` client through the real
    /// `provisionAppClient` procedure — an operator action, not a
    /// generated-CRUD route, for the identical reason `RotateSigningKey`
    /// above is one: `provisionAppClient`'s own `@allow` in
    /// `schema.cstack` is `hasRole('owner') || hasRole('admin')`, and
    /// `GatewayAuth::authenticate` never mints either role for a real
    /// token (no human-login flow exists yet — see `AGENTS.md`'s M1
    /// section). So nothing this deployment can issue over HTTP can call
    /// it; this subcommand calls `Procedures::provision_app_client`
    /// directly, under a hand-built `owner`/`admin` context, the same way
    /// `app/sms-gateway/tests/m1_acceptance_gate_live_postgres.rs` already
    /// does for its own acceptance gate. See #137.
    ///
    /// `ProvisionClientResult` returns `privateKeyPem` exactly once and it
    /// is never stored anywhere in this system (#23/#111) — this command
    /// writes it straight to `--key-out` with `0600` permissions and
    /// refuses to overwrite an existing file, and it is never logged or
    /// printed alongside anything else.
    ProvisionClient {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// The `App.id` this client acts on behalf of. Must already exist
        /// and be active — `provision_client` checks both and refuses
        /// otherwise.
        #[arg(long)]
        app_id: String,

        /// A human-readable label for the resulting `AppClient`, e.g.
        /// `"admin console"` or `"otp sender"`.
        #[arg(long)]
        label: String,

        /// One or more scopes to provision the client with, e.g.
        /// `--scope sms:send --scope sms:read`. At least one is required —
        /// an unscoped client can authenticate but can call nothing.
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,

        /// Which of `provisionAppClient`'s two admitted roles to run the
        /// call under. Both are equally privileged for this call; `owner`
        /// is the default because it's the role every existing live test
        /// already provisions under (`m1_acceptance_gate_live_postgres.rs`,
        /// `provision_app_client_live_postgres.rs`).
        #[arg(long, default_value = "owner")]
        role: String,

        /// Where to write the returned private key, PEM-encoded. Created
        /// with `0600` permissions; this command refuses to run if the
        /// path already exists rather than silently overwriting a key
        /// someone may still be using.
        #[arg(long)]
        key_out: PathBuf,

        /// #134: `Procedures::new` now requires a `HashPepper` unconditionally,
        /// even though `provision_app_client` itself never hashes anything —
        /// only `sendMessage` does. Same flag name and env var as `Serve`'s
        /// own `--hash-pepper`/`SMS_HASH_PEPPER`, so an operator running
        /// this alongside `serve` supplies the identical value once via
        /// their environment rather than learning two different names for
        /// the same secret.
        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: String,
    },
}

/// Writes `pem` to a freshly created file at `path`, `0600` on Unix,
/// refusing to overwrite an existing file (`O_EXCL` via
/// [`std::fs::OpenOptions::create_new`]) — both the mode and the
/// exclusivity are applied atomically at `open(2)` time, so there is no
/// window where the file exists with looser permissions or already-visible
/// contents. See `Command::ProvisionClient`'s own doc comment for why this
/// exists: `ProvisionClientResult::privateKeyPem` is returned exactly once
/// and this is the only place in this system it is ever persisted.
fn write_private_key_pem(path: &std::path::Path, pem: &str) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!(
            "creating {} — refusing to overwrite an existing file, since it may hold a private \
             key still in use",
            path.display()
        )
    })?;
    file.write_all(pem.as_bytes())
        .with_context(|| format!("writing the private key to {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing the private key to {}", path.display()))
}

/// The `system`-role context every OP-adjacent database write in this
/// binary runs under — never handed to a caller, matching
/// `Procedures::sys()`'s own convention.
fn system_context() -> cratestack::CoolContext {
    Principal {
        sub: "sms-gateway:op".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// `Provider.id` for the row matching `provider.key()` — resolved once at
/// startup, not re-checked per DLR callback. Safe to cache: a `Provider`
/// row's own id is immutable once created, and if the row genuinely needs
/// to change (a different key), that's a restart-worthy reconfiguration,
/// not something this route needs to notice live the way key rotation did
/// (see `op.rs`'s own module doc for the contrast — that one needed a live
/// refresh because the *key material* itself changes, not just which row
/// backs a lookup).
///
/// # Errors
///
/// No `Provider` row has this `key` yet — an operator hasn't seeded one,
/// which #34 has no CLI action for (unlike `rotate-signing-key`) since
/// seeding a `Provider` is already `provisionAppClient`-adjacent, ordinary
/// CRUD the admin console (M4) will do, not an ops action this binary
/// should grow its own subcommand for.
async fn resolve_provider_row_id(
    db: &Cratestack,
    sys: &cratestack::CoolContext,
    provider: &dyn SmsProvider,
) -> Result<String> {
    let found = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(provider.key().to_owned()),
        ))
        .limit(1)
        .run(sys)
        .await
        .context("looking up the Provider row for the configured adapter")?;

    found.into_iter().next().map(|row| row.id).with_context(|| {
        format!(
            "no Provider row has key {:?} — seed one before serving DLR callbacks",
            provider.key()
        )
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    // Variables already in the environment win; dotenvy never overwrites.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_gateway=info,sms_api=info,cratestack=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Routes => {
            let routes = sms_api::route_table();
            println!("{} generated routes:", routes.len());
            for (method, path) in routes {
                println!("  {method:<7} {path}");
            }
            Ok(())
        }

        Command::Serve {
            listen,
            database_url,
            max_connections,
            issuer,
            orange_client_id,
            orange_client_secret,
            orange_sender_number,
            orange_base_url,
            hash_pepper,
        } => {
            // #134: validated before anything else in this branch runs —
            // failing loudly on a missing/too-short pepper at startup, not
            // at the first `sendMessage` call. `clap`'s own `env`/required
            // handling already refuses a *missing* value before `main`
            // ever reaches this line; this is the length check clap can't
            // express.
            let pepper = sms_api::HashPepper::new(hash_pepper)
                .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

            let pool = PgPoolOptions::new()
                .max_connections(max_connections)
                .connect(&database_url)
                .await
                .context("connecting to Postgres")?;

            let db = Cratestack::builder(pool).build();
            let sys = system_context();

            let (signing, jwks) = sms_auth::op::load_signing_keys(&db, &sys, &issuer)
                .await
                .context(
                    "loading OP signing keys — run `sms-gateway rotate-signing-key` if this is \
                     a fresh database",
                )?;
            let op_store =
                sms_auth::op::machine_only_store(std::sync::Arc::new(db.clone()), sys.clone());
            let op_config = sms_auth::op::machine_only_config(issuer.clone());
            let op_state = op::OpState::new(op_store, signing, op_config, jwks);
            // Keeps a rotate-signing-key run against this already-running
            // process from silently never taking effect — see op.rs's own
            // module doc.
            op::spawn_key_refresh(
                op_state.clone(),
                db.clone(),
                sys.clone(),
                issuer.clone(),
                op::DEFAULT_KEY_REFRESH_INTERVAL,
            );

            let mut orange_config = sms_provider_orange_cm::OrangeCmConfig::production(
                orange_client_id,
                orange_client_secret,
                orange_sender_number,
            );
            orange_config.base_url = orange_base_url;
            let provider: Arc<dyn SmsProvider> =
                Arc::new(sms_provider_orange_cm::OrangeCmProvider::new(orange_config));
            let provider_row_id = resolve_provider_row_id(&db, &sys, provider.as_ref()).await?;
            let dlr_router = dlr::router(db.clone(), sys, provider, provider_row_id);

            let auth = GatewayAuth::new(db.clone(), format!("{issuer}/jwks.json"), issuer);
            let app = sms_api::router(db, auth, pepper)
                .merge(op::router(op_state))
                .merge(dlr_router);

            let listener = tokio::net::TcpListener::bind(&listen)
                .await
                .with_context(|| format!("binding {listen}"))?;
            info!(listen = %listen, "sms-gateway listening");

            cratestack::axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .context("serving HTTP")?;
            Ok(())
        }

        Command::RotateSigningKey { database_url } => {
            // Found live prepping #36 (this repo's first time ever running
            // this command against a real database): `max_connections(1)`
            // deadlocks with `CoolError::Database("pool timed out waiting
            // for an open connection")` on the very first
            // `OauthSigningKey::create` — reproduced twice, and confirmed
            // fixed at exactly `max_connections(2)`, one shy of that. Most
            // likely cause: `OauthSigningKey`'s `@@audit` writes its audit
            // row on a connection acquired separately from (and held
            // concurrently with) the row write's own, rather than sharing
            // one — unconfirmed against `cratestack`'s own generated code,
            // but the connection-count boundary itself is empirically
            // solid. A one-shot CLI command needs no real concurrency, so
            // this stays small — a modest margin above the confirmed
            // minimum, not `serve`'s pooled-server-sized default, in case
            // a rotation with several still-active-but-expiring keys to
            // deactivate needs more than one such pair in flight.
            let pool = PgPoolOptions::new()
                .max_connections(4)
                .connect(&database_url)
                .await
                .context("connecting to Postgres")?;
            let db = Cratestack::builder(pool).build();

            let id = sms_auth::op::rotate_signing_key(
                &db,
                &system_context(),
                sms_auth::op::ROTATION_OVERLAP,
            )
            .await
            .context("rotating the OP signing key")?;
            println!("rotated: new signing key {id} is now active");
            println!(
                "the previous key keeps publishing in JWKS for {} minutes",
                sms_auth::op::ROTATION_OVERLAP.num_minutes()
            );
            Ok(())
        }

        command @ Command::ProvisionClient { .. } => provision_client_command(command).await,
    }
}

/// `Command::ProvisionClient`'s body, pulled out of `main`'s own `match`
/// purely to stay under `clippy::too_many_lines` — see that variant's own
/// doc comment for what this does and why it exists. Takes the whole
/// matched `Command` (rather than its fields individually) so the
/// multi-line destructure lives here instead of adding more lines to the
/// already-large `match` in `main`; the `unreachable!()` below can never
/// fire because the only caller is `main`'s own `command @
/// Command::ProvisionClient { .. }` guard.
async fn provision_client_command(command: Command) -> Result<()> {
    let Command::ProvisionClient {
        database_url,
        app_id,
        label,
        scopes,
        role,
        key_out,
        hash_pepper,
    } = command
    else {
        unreachable!("only ever called with Command::ProvisionClient")
    };

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — provisionAppClient's own @allow admits \
             nothing else, got {role:?}"
        );
    }
    // Refuse up front, before touching the database at all, so a typo'd
    // --key-out never causes a real provisioning call (and a real,
    // now-orphaned private key) that this process then fails to hand back
    // to the operator.
    if key_out.exists() {
        bail!(
            "{} already exists — refusing to overwrite a file that may hold a private key \
             still in use; pass a different --key-out",
            key_out.display()
        );
    }
    // #134: validated up front for the same reason `Serve` validates its
    // own copy before doing anything else — `provision_app_client` never
    // hashes anything itself, but `Procedures::new` takes an unconditional
    // `HashPepper` regardless, so a bad pepper must fail before a real
    // provisioning call happens, not after.
    let pepper = sms_api::HashPepper::new(hash_pepper)
        .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

    // Same conservative pool size as `RotateSigningKey`, and for the same
    // reason: this is a one-shot CLI command writing two `@@audit`-backed
    // rows (`AppClient`, `OauthClient`) in one transaction, the same shape
    // of write that command's own comment found deadlocks at
    // `max_connections(1)`. Never empirically re-tested at 1 here, so the
    // same modest margin is kept rather than assumed safe at the minimum.
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let ctx = Principal {
        sub: format!("sms-gateway:provision-client:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let procedures = Procedures::new(pepper);
    let provisioned = procedures
        .provision_app_client(
            &db,
            &ctx,
            provision_app_client::Args {
                args: ProvisionClientInput {
                    appId: app_id,
                    label,
                    scopes,
                },
            },
        )
        .await
        .context("provisioning the client")?;

    // Destructured immediately and never reassembled: nothing past this
    // point may hold, log, or `{:?}`-print `provisioned` as a whole — see
    // `write_private_key_pem`'s own doc for why the file below is the
    // only place this value's private key is ever allowed to land.
    let client_id = provisioned.clientId;
    let private_key_pem = provisioned.privateKeyPem;

    write_private_key_pem(&key_out, &private_key_pem)?;

    println!("provisioned client: {client_id}");
    println!("private key written to: {}", key_out.display());
    println!();
    println!("paste into the console (or any other machine caller)'s environment:");
    println!("  SMS_CONSOLE_CLIENT_ID={client_id}");
    println!("  SMS_CONSOLE_PRIVATE_KEY_PATH={}", key_out.display());
    Ok(())
}

/// Resolve on SIGINT *or* SIGTERM so in-flight requests finish.
///
/// `ctrl_c()` alone only catches SIGINT. §9.2 deploys this as a Docker
/// container, and `docker stop` / `kubectl rollout restart` send SIGTERM
/// first, SIGKILL only after the grace period elapses — SIGINT is never
/// sent in that path at all. Missing SIGTERM here would mean this branch
/// never fires under the deployment §9.2 actually describes, and the
/// process would always hit the force-kill timeout instead, silently,
/// since a container restarting slightly late looks identical to one
/// restarting correctly.
///
/// Unix-only because `tokio::signal::unix` is: §9.2's deployment is Docker
/// Compose on a single VM, never Windows, so a `cfg(unix)` split with a
/// SIGINT-only fallback elsewhere costs nothing this binary needs.
///
/// Milestone 2 adds the advisory-lock release here — `Drop` cannot do it,
/// because releasing needs an `await`.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("installing a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("shutdown signal received");
}
