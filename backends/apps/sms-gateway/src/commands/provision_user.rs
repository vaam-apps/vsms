//! `Command::ProvisionUser` (#194) — see that variant's own doc comment in
//! `main.rs` for what this does and why it exists. It makes a `User` row and
//! no credential: since VS-4 a person signs in by pairing a console key.

use anyhow::{Context, Result};
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, CreateUserInput};
use sms_api::{Principal, PrincipalKind};

/// `Command::ProvisionUser`'s flags. See `Command::ProvisionUser`'s own
/// doc comment in `main.rs` — the enum variant carries the "why", this
/// struct only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct ProvisionUserArgs {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    #[arg(long)]
    pub(crate) email: String,

    #[arg(long)]
    pub(crate) display_name: String,

    /// Must already exist — `User.roleKey` is a foreign key to
    /// `Role.key`, and this command does not create roles.
    ///
    /// §5.2's six built-in roles (`owner`, `admin`, `operator`,
    /// `developer`, `auditor`, `support`) **are** seeded, by
    /// `0002_bootstrap`, so on any migrated database one of those
    /// keys works directly. That was not true when this command
    /// landed: nothing seeded `roles` at all, so this argument could
    /// not be satisfied on a fresh database by any means, and the
    /// first human account was unreachable — the chicken-and-egg this
    /// doc comment used to describe. See `docs/architecture.md`
    /// §2.10's own note on the seed.
    ///
    /// `system` is deliberately not among them and cannot be created:
    /// `roles_key_not_reserved_check` rejects it.
    #[arg(long)]
    pub(crate) role_key: String,
}

/// The `User` provisioning logic, shared by [`provision_user_command`] and
/// `bootstrap_command` — see `seed_dispatch_core`'s (`commands::seed_dispatch`)
/// own doc for why this split exists.
///
/// Both writes (`User` create, then `User` update-subject) run in one
/// `run_in_isolated_tx` transaction, so a failure between them cannot leave a
/// row whose `subject` is still the `pending-…` placeholder.
///
/// Returns `Ok(None)` rather than an error on a duplicate `email` (a
/// `23505` on `users_email_key`, narrowed by constraint name so an unrelated
/// `23505` propagates loudly instead of being folded into "already
/// exists") — the same "already-provisioned is success, not failure" idiom
/// every other seed/provision function in this file uses, and specifically
/// what `bootstrap_command` needs to stay a clean no-op re-run against an
/// already-bootstrapped deployment. Otherwise the new user's id.
pub(crate) async fn create_console_user_if_absent(
    db: &Cratestack,
    email: &str,
    display_name: &str,
    role_key: &str,
) -> Result<Option<String>> {
    // `User.create`'s own `@@allow` is `hasRole('owner') ||
    // hasRole('admin')` — never `hasRole('system')` — so this needs a
    // human-role-shaped context, not the internal system one.
    let ctx = Principal {
        sub: "sms-gateway:provision-user:owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context();

    let email_owned = email.to_owned();
    let display_name_owned = display_name.to_owned();
    let role_key_owned = role_key.to_owned();

    cratestack::run_in_isolated_tx(
        db.pool(),
        cratestack::TransactionIsolation::Serializable,
        |mut tx| {
            let ctx = &ctx;
            let email = email_owned.clone();
            let display_name = display_name_owned.clone();
            let role_key = role_key_owned.clone();
            async move {
                let user = match db
                    .user()
                    .create(CreateUserInput {
                        // `subject` is this row's own id (it is vsms's own
                        // subject for the person; an upstream identity lives in
                        // `FederatedLink`). `db.user().create` doesn't know its
                        // own generated id ahead of the call, so this writes a
                        // unique placeholder (subject is @unique — a fixed
                        // literal here would make a second concurrent run
                        // collide on it before either gets to the corrective
                        // update below) and corrects it in a second update, in
                        // the same transaction.
                        subject: format!("pending-{}", cratestack::uuid::Uuid::new_v4()),
                        email,
                        displayName: display_name,
                        roleKey: role_key,
                        lastLoginAt: None,
                        deletedAt: None,
                    })
                    .run_in_tx(&mut tx, ctx)
                    .await
                {
                    Ok(created) => created.value,
                    Err(e)
                        if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION)
                            && e.db_constraint() == Some("users_email_key") =>
                    {
                        return Ok((None, tx));
                    }
                    Err(e) => return Err(e),
                };

                db.user()
                    .update(user.id.clone())
                    .set(sms_api::schema::UpdateUserInput {
                        subject: Some(user.id.clone()),
                        ..Default::default()
                    })
                    // #59: User is @version'd. cratestack refuses a
                    // versioned update with no If-Match at runtime.
                    .if_match(user.version)
                    .run_in_tx(&mut tx, ctx)
                    .await?;

                Ok((Some(user.id), tx))
            }
        },
    )
    .await
    .context("creating the User row — check that --role-key names an existing Role")
}

/// `Command::ProvisionUser`'s body (#194) — see that variant's own doc
/// comment for what this does and why it exists. Takes
/// [`ProvisionUserArgs`] directly rather than the whole `Command`: `main`'s
/// own dispatch already extracted it from `Command::ProvisionUser` at the
/// match site.
pub(crate) async fn provision_user_command(args: ProvisionUserArgs) -> Result<()> {
    let ProvisionUserArgs {
        database_url,
        email,
        display_name,
        role_key,
    } = args;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let Some(user_id) =
        create_console_user_if_absent(&db, &email, &display_name, &role_key).await?
    else {
        println!("a User with email {email:?} already exists — nothing to do");
        return Ok(());
    };

    println!("provisioned user: {email} (id={user_id})");
    println!(
        "no credential exists to hand over: pair a browser key with `sms-gateway \
         pair-console-key --user {email} --public-key <key>`, or through a vaam admin phone."
    );
    Ok(())
}
