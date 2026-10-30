//! `Command::SeedConsoleClient` (VSA-4a) - see that variant's own doc comment in
//! `main.rs` for what this registers and why the row is checked, not trusted.

use anyhow::{Context, Result};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::Cratestack;
use sms_api::system_context;
use sms_auth::console_client::{Seeded, seed_console_client};
use sms_auth::gate::HumanLogin;

/// `Command::SeedConsoleClient`'s flags. See `Command::SeedConsoleClient`'s own
/// doc comment in `main.rs` - the enum variant carries the "why", this struct only
/// carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct SeedConsoleClientArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// The YAML deployment configuration: its `human_login.console_client` is
    /// what the row is made to match, and a deployment that does not serve
    /// `human_login` has no console client to register, so this command refuses.
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,
}

/// `Command::SeedConsoleClient`'s body - see that variant's own doc comment.
/// Idempotent and reconciling: created when missing, brought into line when it
/// differs, left alone when it matches. Takes [`SeedConsoleClientArgs`] directly
/// rather than the whole `Command`: `main`'s own dispatch already extracted it.
pub(crate) async fn seed_console_client_command(args: SeedConsoleClientArgs) -> Result<()> {
    let SeedConsoleClientArgs {
        database_url,
        config,
    } = args;

    // Before the database is touched: no `human_login`, no console client.
    let loaded = config
        .load(sms_config::Binary::Operator)
        .context("loading the deployment configuration")?;
    HumanLogin::from_methods(&loaded.auth_methods).require()?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    seed_console_client_core(&db, &loaded).await
}

/// The seeding, shared by [`seed_console_client_command`] and `bootstrap_command`
/// (see `seed_dispatch_core`'s doc for why this split exists). `loaded` has
/// already passed the [`HumanLogin`] gate.
pub(crate) async fn seed_console_client_core(
    db: &Cratestack,
    loaded: &sms_config::Config,
) -> Result<()> {
    let human = loaded
        .human_login
        .as_ref()
        .context("human_login: is missing although human_login is served")?;
    let console = &human.console_client;
    let seeded = seed_console_client(db, &system_context("sms-gateway:op"), console)
        .await
        .context("seeding the console OauthClient row")?;
    match seeded {
        Seeded::Created => println!("registered the console client {:?}", console.client_id),
        Seeded::Reconciled(changes) => {
            println!(
                "reconciled the console client {:?} with human_login.console_client:",
                console.client_id
            );
            for change in changes {
                println!("  was wrong: {change}");
            }
        }
        Seeded::Unchanged => println!(
            "the console client {:?} already matches human_login.console_client - nothing to do",
            console.client_id
        ),
    }
    Ok(())
}
