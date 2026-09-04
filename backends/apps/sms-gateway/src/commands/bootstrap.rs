//! `Command::Bootstrap` — see that variant's own doc comment in `main.rs`
//! for the whole chain and the R4 console-optional validation. Every step
//! here calls the exact same function its own standalone subcommand does.

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateProviderInput, ProviderKind, oauth_signing_key as oauth_signing_key_filter,
};
use sms_api::{Principal, PrincipalKind};

use crate::commands::provision_user::create_console_user_if_absent;
use crate::commands::seed_console_client::seed_console_client_core;
use crate::commands::seed_dispatch::seed_dispatch_core;
use sms_api::system_context;

/// `Command::Bootstrap`'s flags. See `Command::Bootstrap`'s own doc
/// comment in `main.rs` — the enum variant carries the "why", this struct
/// only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct BootstrapArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,

    /// Passed through to the `seed-console-client` step verbatim —
    /// see that variant's own doc for why it must match
    /// `sms-gateway serve --console-client-id` and `admin`'s own
    /// `SMS_CONSOLE_OIDC_CLIENT_ID` exactly. Irrelevant, and never
    /// read, when `--console-redirect-uri` is absent.
    #[arg(
        long,
        env = "SMS_CONSOLE_OIDC_CLIENT_ID",
        default_value = sms_api::DEFAULT_CONSOLE_CLIENT_ID
    )]
    pub(crate) console_client_id: String,

    /// Passed through to the `seed-console-client` step verbatim —
    /// must equal `{ADMIN_BASE_URL}/api/auth/callback` exactly (RFC
    /// 6749 §3.1.2, whole-string comparison). Optional: omit
    /// entirely for a backend-only deployment (R4) — doing so skips
    /// both this step and the owner-account step below, printing
    /// why rather than silently doing nothing.
    #[arg(long)]
    pub(crate) console_redirect_uri: Option<String>,

    /// Passed through to the `provision-user` step as `--email`.
    /// Optional, but requires `--console-redirect-uri` to be given
    /// alongside it — `bootstrap_command` refuses to start with a
    /// named error otherwise, rather than silently provisioning an
    /// owner account with no console to sign into.
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
}

/// Bootstrap step 1/4 — see `bootstrap_command`'s own doc for the whole
/// chain. Pulled out purely to keep `bootstrap_command` itself under
/// `clippy::too_many_lines`, the same reason every other multi-step
/// command function in this file is already split this way.
///
/// Never rotates unconditionally: unlike every other step in this chain,
/// rotation is not idempotent — re-running it against an
/// already-bootstrapped deployment would mint a brand-new key and start
/// the previous one's `ROTATION_OVERLAP` countdown early, exactly the
/// silent-on-every-upgrade trap `values.yaml`'s own `rotateSigningKey`
/// Helm hook comment already documents for why that hook is deliberately
/// *not* `pre-upgrade`. So this checks for an existing `active` row
/// first, the same existence check `load_signing_keys` itself makes
/// before erroring, rather than calling `sms_auth::op::rotate_signing_key`
/// (the real function every `rotate-signing-key` invocation goes through)
/// unconditionally.
async fn bootstrap_step_signing_key(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
) -> Result<()> {
    println!("== bootstrap: step 1/4 — OP signing key ==");
    let has_active_signing_key = !db
        .oauth_signing_key()
        .find_many()
        .where_expr(FilterExpr::from(
            oauth_signing_key_filter::active().is_true(),
        ))
        .limit(1)
        .run(sys)
        .await
        .context("checking for an existing active OauthSigningKey")?
        .is_empty();
    if has_active_signing_key {
        println!("an active OauthSigningKey already exists — skipping rotate-signing-key");
        return Ok(());
    }
    let id = sms_auth::op::rotate_signing_key(db, sys, sms_auth::op::ROTATION_OVERLAP).await?;
    println!("rotated: new signing key {id} is now active");
    Ok(())
}

/// Bootstrap step 2/4 — see `bootstrap_command`'s own doc. Same defaults
/// `Command::SeedDispatch`'s own `#[arg(...)]` attributes fall back to —
/// `bootstrap` exposes no flags for these, matching `deployment.adoc`
/// step 3's own "no flags are required here" note (in its `seed-dispatch`
/// sub-step).
async fn bootstrap_step_seed_dispatch(db: &Cratestack) -> Result<()> {
    println!("== bootstrap: step 2/4 — orange_cm Provider + catch-all Route ==");
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

/// Bootstrap steps 3/4 — see `bootstrap_command`'s own doc. Split out
/// purely to keep that function under `clippy::too_many_lines`, and
/// because both steps share one precondition
/// (`console_redirect_uri.is_some()`) that's simplest to reason about
/// together: R4's own backend-only case skips both in one place rather
/// than two separately-reasoned-about branches.
async fn bootstrap_step_console(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    console_client_id: &str,
    console_redirect_uri: Option<&str>,
    owner_email: Option<&str>,
    owner_display_name: Option<&str>,
    owner_role_key: &str,
) -> Result<()> {
    println!("== bootstrap: step 3/4 — sms-console OauthClient ==");
    let Some(redirect_uri) = console_redirect_uri else {
        println!("skipped — no --console-redirect-uri given (backend-only deployment)");
        println!("== bootstrap: step 4/4 — first operator account ==");
        println!("skipped — no --console-redirect-uri given (backend-only deployment)");
        return Ok(());
    };
    seed_console_client_core(db, sys, console_client_id, redirect_uri).await?;

    println!("== bootstrap: step 4/4 — first operator account ==");
    let (Some(email), Some(display_name)) = (owner_email, owner_display_name) else {
        println!("skipped — no --owner-email given");
        return Ok(());
    };
    match create_console_user_if_absent(db, sys, email, display_name, owner_role_key).await? {
        Some((user_id, password)) => {
            println!("provisioned user: {email} (id={user_id})");
            println!("one-time password (never stored, never shown again): {password}");
            println!(
                "share this over a channel the recipient controls, not this command's own \
                 stdout log."
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
/// standalone subcommand does ([`bootstrap_step_signing_key`]/
/// [`bootstrap_step_seed_dispatch`]/[`bootstrap_step_console`]), over one
/// shared pool, rather than re-deriving any of their logic. Takes
/// [`BootstrapArgs`] directly rather than the whole `Command`: `main`'s
/// own dispatch already extracted it from `Command::Bootstrap` at the
/// match site.
pub(crate) async fn bootstrap_command(args: BootstrapArgs) -> Result<()> {
    let BootstrapArgs {
        database_url,
        console_client_id,
        console_redirect_uri,
        owner_email,
        owner_display_name,
        owner_role_key,
    } = args;

    // R4: `--owner-email` with no `--console-redirect-uri` would
    // silently provision an operator account with nothing to sign
    // into — refuse before connecting to the database at all, the same
    // "validate first" discipline `provision_client_command`'s own
    // `--role` check already uses.
    if owner_email.is_some() && console_redirect_uri.is_none() {
        bail!(
            "--owner-email requires --console-redirect-uri — both are needed together to \
             provision the console's human-login half of bootstrap. Omit both for a \
             backend-only deployment (R4), or supply both."
        );
    }
    if owner_email.is_some() && owner_display_name.is_none() {
        bail!("--owner-email requires --owner-display-name — both are required together");
    }

    // Same conservative pool size as every other one-shot command in this
    // file, and for the same reason (rotate_signing_key_command's own
    // comment) — this one just reuses it across four writes instead of
    // one or two.
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:op");

    bootstrap_step_signing_key(&db, &sys).await?;
    bootstrap_step_seed_dispatch(&db).await?;
    bootstrap_step_console(
        &db,
        &sys,
        &console_client_id,
        console_redirect_uri.as_deref(),
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
