//! `Command::RotateSigningKey` — see that variant's own doc comment in
//! `main.rs` for what this does and why it's an operator action rather
//! than a generated-CRUD route.

use anyhow::{Context, Result};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::Cratestack;
use sms_api::system_context;
use sms_auth::gate::Provider;

/// `Command::RotateSigningKey`'s flags.
#[derive(Debug, clap::Args)]
pub(crate) struct RotateSigningKeyArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// The YAML deployment configuration (VSA-3a): it says whether this
    /// deployment mounts the provider at all (`machine_tokens` or `human_login`
    /// served), and how long a rotated-out key keeps publishing
    /// (`op.signing_key.rotation_overlap_secs`).
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,
}

/// `Command::RotateSigningKey`'s body, pulled out of `main`'s own `match`
/// for the same reason `provision_client_command` (`commands::provision_client`)
/// is — see that function's doc.
pub(crate) async fn rotate_signing_key_command(args: RotateSigningKeyArgs) -> Result<()> {
    let config = args
        .config
        .load(sms_config::Binary::Operator)
        .context("loading the deployment configuration")?;
    // Before the database is touched: a deployment that mounts no provider has
    // nothing to sign for, and a key minted anyway would look like a rotation.
    // Either method rides the provider, so either is a reason to rotate its key.
    Provider::from_methods(&config.auth_methods).require()?;
    let op = config
        .op
        .as_ref()
        .context("op: is missing although the provider is served")?;
    let overlap = chrono::Duration::seconds(
        i64::try_from(op.signing_key.rotation_overlap_secs)
            .context("op.signing_key.rotation_overlap_secs does not fit an i64")?,
    );

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
        .connect(&args.database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let id = sms_auth::op::rotate_signing_key(&db, &system_context("sms-gateway:op"), overlap)
        .await
        .context("rotating the OP signing key")?;
    println!("rotated: new signing key {id} is now active");
    println!(
        "the previous key keeps publishing in JWKS for {} minutes",
        overlap.num_minutes()
    );
    Ok(())
}
