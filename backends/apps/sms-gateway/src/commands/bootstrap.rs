//! `Command::Bootstrap` — see that variant's own doc comment in `main.rs`
//! for the whole chain. Every step here calls the exact same function its own
//! standalone subcommand does.

use anyhow::{Context, Result, bail};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, CreateProviderInput, ProviderKind};
use sms_api::{Principal, PrincipalKind};

use crate::commands::provision_user::create_console_user_if_absent;
use crate::commands::seed_dispatch::seed_dispatch_core;

/// `Command::Bootstrap`'s flags. See `Command::Bootstrap`'s own doc
/// comment in `main.rs` — the enum variant carries the "why", this struct
/// only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct BootstrapArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// Passed through to the `provision-user` step as `--email`.
    /// Optional: omit it for a backend-only deployment (R4), which
    /// skips the owner-account step and says so rather than silently
    /// doing nothing. Requires `--owner-display-name`.
    #[arg(long)]
    pub(crate) owner_email: Option<String>,

    /// Passed through to the `provision-user` step as
    /// `--display-name`. Required together with `--owner-email`
    /// (both or neither).
    #[arg(long)]
    pub(crate) owner_display_name: Option<String>,

    /// Passed through to the `provision-user` step as `--role-key`.
    #[arg(long, default_value = "owner")]
    pub(crate) owner_role_key: String,

    /// Skip step 1 (the hardcoded `orange_cm` `Provider` + catch-all
    /// `Route`). See `Command::Bootstrap`'s own doc for when this is the
    /// right call.
    #[arg(long)]
    pub(crate) skip_seed_dispatch: bool,
}

/// Bootstrap step 1/2 — see `bootstrap_command`'s own doc. Same defaults
/// `Command::SeedDispatch`'s own `#[arg(...)]` attributes fall back to —
/// `bootstrap` exposes no flags for these, matching `deployment.adoc`
/// step 3's own "no flags are required here" note (in its `seed-dispatch`
/// sub-step).
async fn bootstrap_step_seed_dispatch(db: &Cratestack) -> Result<()> {
    println!("== bootstrap: step 1/2 — orange_cm Provider + catch-all Route ==");
    let ctx = Principal {
        sub: "sms-gateway:bootstrap:seed-dispatch".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    seed_dispatch_core(
        db,
        &ctx,
        CreateProviderInput {
            key: "orange_cm".to_owned(),
            displayName: "Orange Cameroon SMS API".to_owned(),
            kind: ProviderKind::orange_cm_http,
            config: "{}".to_owned(),
            credentialRef: "env:ORANGE_CM_CLIENT_SECRET".to_owned(),
            maxTps: 10.0,
            maxDailySubmissions: 100_000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: cratestack::Decimal::ZERO,
            healthCheckedAt: None,
            circuitOpenUntil: None,
        },
    )
    .await
}

/// Bootstrap step 2/2 — see `bootstrap_command`'s own doc. The first
/// operator account is a `User` row and nothing else: the person signs in by
/// pairing a console key (`pair-console-key`, or a vaam admin phone), so
/// there is no credential to hand over.
async fn bootstrap_step_owner(
    db: &Cratestack,
    owner_email: Option<&str>,
    owner_display_name: Option<&str>,
    owner_role_key: &str,
) -> Result<()> {
    println!("== bootstrap: step 2/2 — first operator account ==");
    let (Some(email), Some(display_name)) = (owner_email, owner_display_name) else {
        println!("skipped — no --owner-email given (backend-only deployment)");
        return Ok(());
    };
    match create_console_user_if_absent(db, email, display_name, owner_role_key).await? {
        Some(user_id) => {
            println!("provisioned user: {email} (id={user_id})");
            println!(
                "no credential exists to hand over: pair a browser key with `sms-gateway \
                 pair-console-key --user {email} --public-key <key>`, or through a vaam admin \
                 phone."
            );
        }
        None => {
            println!("a User with email {email:?} already exists — skipping provision-user");
        }
    }
    Ok(())
}

/// `Command::Bootstrap`'s body — see that variant's own doc comment for
/// what this chains and why, including the R4 console-optional
/// validation this function enforces before touching the database at
/// all. Every step below calls the exact same function its own
/// standalone subcommand does ([`bootstrap_step_seed_dispatch`]/
/// [`bootstrap_step_owner`]), over one shared pool, rather than
/// re-deriving any of their logic. Takes [`BootstrapArgs`] directly rather
/// than the whole `Command`: `main`'s own dispatch already extracted it from
/// `Command::Bootstrap` at the match site.
pub(crate) async fn bootstrap_command(args: BootstrapArgs) -> Result<()> {
    let BootstrapArgs {
        database_url,
        owner_email,
        owner_display_name,
        owner_role_key,
        skip_seed_dispatch,
    } = args;

    // Refused before connecting to the database at all, the same
    // "validate first" discipline `register_client_command`'s own `--role`
    // check uses.
    if owner_email.is_some() && owner_display_name.is_none() {
        bail!("--owner-email requires --owner-display-name — both are required together");
    }
    if owner_email.is_none() && owner_display_name.is_some() {
        bail!("--owner-display-name requires --owner-email — both are required together");
    }

    // Same conservative pool size as every other one-shot command in this
    // file.
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    if skip_seed_dispatch {
        // Not a silent no-op: the operator asked for this, but the step's
        // header and the follow-up are printed like every other skip so
        // the output still reads as the two-step chain it is.
        println!("== bootstrap: step 1/2 — orange_cm Provider + catch-all Route ==");
        println!(
            "skipped — run seed-dispatch for the provider you will send through \
             (--skip-seed-dispatch given)"
        );
    } else {
        bootstrap_step_seed_dispatch(&db).await?;
    }
    bootstrap_step_owner(
        &db,
        owner_email.as_deref(),
        owner_display_name.as_deref(),
        &owner_role_key,
    )
    .await?;

    println!();
    println!(
        "bootstrap complete — `sms-gateway serve`/`sms-worker` can now start against this \
         database. Run `sms-gateway create-app` next for a real production App (its own \
         quota/allowlist is a decision this command doesn't make for you)."
    );
    Ok(())
}
