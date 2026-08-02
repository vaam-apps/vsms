//! The SMS gateway API server.

mod dlr;
mod op;

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::FilterExpr;
use sms_api::schema::{provider as provider_filter, Cratestack};
use sms_api::{GatewayAuth, Principal, PrincipalKind};
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
        } => {
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
            let app = sms_api::router(db, auth)
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
            let pool = PgPoolOptions::new()
                .max_connections(1)
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
    }
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
