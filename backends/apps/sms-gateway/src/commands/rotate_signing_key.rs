//! `Command::RotateSigningKey` — see that variant's own doc comment in
//! `main.rs` for what this does and why it's an operator action rather
//! than a generated-CRUD route.

use anyhow::{Context, Result};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::Cratestack;

use crate::commands::common::system_context;

/// `Command::RotateSigningKey`'s flags.
#[derive(Debug, clap::Args)]
pub(crate) struct RotateSigningKeyArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,
}

/// `Command::RotateSigningKey`'s body, pulled out of `main`'s own `match`
/// for the same reason `provision_client_command` (`commands::provision_client`)
/// is — see that function's doc.
pub(crate) async fn rotate_signing_key_command(database_url: String) -> Result<()> {
    // Found live prepping #36 (this repo's first time ever running
    // this command against a real database): `max_connections(1)`
    // deadlocks with `CratestackError::Database("pool timed out waiting
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

    let id =
        sms_auth::op::rotate_signing_key(&db, &system_context(), sms_auth::op::ROTATION_OVERLAP)
            .await
            .context("rotating the OP signing key")?;
    println!("rotated: new signing key {id} is now active");
    println!(
        "the previous key keeps publishing in JWKS for {} minutes",
        sms_auth::op::ROTATION_OVERLAP.num_minutes()
    );
    Ok(())
}
