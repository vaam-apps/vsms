//! `Command::Bootstrap` — see that variant's own doc comment in `main.rs`
//! for the whole chain. Every step here calls the exact same function its own
//! standalone subcommand does.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, CreateProviderInput, ProviderKind};
use sms_api::{Principal, PrincipalKind};
use sms_auth::gate::HumanLogin;

use crate::commands::provision_user::{
    announce_password, create_console_user_if_absent, issue_password,
};
use crate::commands::seed_console_client::seed_console_client_core;
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

    /// Also give the first owner a password (VSA-4a), generated here and written
    /// to this path (`0600`, `O_EXCL`), never printed, and register the console
    /// client (`seed-console-client`) in the same run. Needs `--owner-email` and a
    /// deployment that serves `human_login`; refused before the database is
    /// touched otherwise. Without it `bootstrap` reads no configuration, as
    /// before.
    #[arg(long, requires = "owner_email")]
    pub(crate) owner_password_out: Option<PathBuf>,

    /// The YAML deployment configuration, read only for `--owner-password-out`.
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,
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
/// operator account is a `User` row; the person signs in by pairing a console key
/// (`pair-console-key`, or a vaam admin phone), or, with `--owner-password-out`
/// on a deployment that serves `human_login`, with the password written there.
async fn bootstrap_step_owner(
    db: &Cratestack,
    owner_email: Option<&str>,
    owner_display_name: Option<&str>,
    owner_role_key: &str,
    password_out: Option<&Path>,
) -> Result<()> {
    println!("== bootstrap: step 2/2 — first operator account ==");
    let (Some(email), Some(display_name)) = (owner_email, owner_display_name) else {
        println!("skipped — no --owner-email given (backend-only deployment)");
        return Ok(());
    };
    let created = create_console_user_if_absent(db, email, display_name, owner_role_key).await?;
    match (&created, password_out) {
        (Some(user_id), None) => {
            println!("provisioned user: {email} (id={user_id})");
            println!(
                "no credential exists to hand over: pair a browser key with `sms-gateway \
                 pair-console-key --user {email} --public-key <key>`, or through a vaam admin \
                 phone."
            );
        }
        (None, None) => {
            println!("a User with email {email:?} already exists — skipping provision-user");
        }
        (Some(user_id), Some(out)) => {
            println!("provisioned user: {email} (id={user_id})");
            let issued = issue_password(db, user_id, out, false).await?;
            announce_password(issued, email, out);
        }
        (None, Some(out)) => {
            println!("a User with email {email:?} already exists — skipping provision-user");
            // The account was made by an earlier run that gave it no password (or
            // one that did): a password is set only where none exists, and an
            // existing one is refused, never replaced by a re-run (that is
            // `provision-user --reset`).
            let user_id = crate::commands::provision_user::user_id_by_email(db, email)
                .await?
                .with_context(|| format!("the User with email {email:?} vanished"))?;
            let issued = issue_password(db, &user_id, out, false).await?;
            announce_password(issued, email, out);
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
        owner_password_out,
        config,
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

    // The password path needs the deployment to serve the sign-in, and its file
    // not to exist: both refused before the database is touched.
    let loaded = if let Some(out) = &owner_password_out {
        let loaded = config
            .load(sms_config::Binary::Operator)
            .context("loading the deployment configuration")?;
        HumanLogin::from_methods(&loaded.auth_methods).require()?;
        if out.exists() {
            bail!(
                "{} already exists - refusing to overwrite a file that may hold a password \
                 still in use; pass a different --owner-password-out",
                out.display()
            );
        }
        Some(loaded)
    } else {
        None
    };

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
    if let Some(loaded) = &loaded {
        // The console client the owner will sign in through, so the one run
        // leaves a sign-in that works.
        println!("== bootstrap: console client (human_login is served) ==");
        seed_console_client_core(&db, loaded).await?;
    }
    bootstrap_step_owner(
        &db,
        owner_email.as_deref(),
        owner_display_name.as_deref(),
        &owner_role_key,
        owner_password_out.as_deref(),
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
