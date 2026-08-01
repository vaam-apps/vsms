//! The SMS gateway API server.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cratestack::sqlx::postgres::PgPoolOptions;
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
    },
    /// Print the generated route table and exit. Needs no database.
    Routes,
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
        } => {
            let pool = PgPoolOptions::new()
                .max_connections(max_connections)
                .connect(&database_url)
                .await
                .context("connecting to Postgres")?;

            let db = sms_api::schema::Cratestack::builder(pool).build();
            let db_arc = std::sync::Arc::new(db.clone());

            let sys_ctx = sms_api::auth::Principal {
                sub: "system".to_string(),
                kind: sms_api::auth::PrincipalKind::App,
                role: "system".to_string(),
                app_id: "system".to_string(),
            }
            .into_context();

            let issuer_url = format!("http://{listen}");

            let op_state = sms_auth::provider::setup_op_state(
                db_arc.clone(),
                sys_ctx.clone(),
                issuer_url.clone(),
            )
            .await
            .context("setting up OP state")?;

            let op_router = sms_auth::provider::op_router(op_state);

            let oidc_validator =
                sms_api::auth::OidcValidator::new(&issuer_url, db_arc.clone(), sys_ctx.clone());
            let api_router = sms_api::router(db, oidc_validator);

            let app = cratestack::axum::Router::new()
                .merge(op_router)
                .merge(api_router);

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
    }
}

/// Resolve on SIGINT so in-flight requests finish.
///
/// Milestone 2 adds the advisory-lock release here — `Drop` cannot do it,
/// because releasing needs an `await`.
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received");
}
