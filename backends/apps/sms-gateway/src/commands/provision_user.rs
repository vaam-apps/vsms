//! `Command::ProvisionUser` (#194) — see that variant's own doc comment in
//! `main.rs` for what this does, why it exists, and why the password is
//! generated rather than accepted as a flag.

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateUserCredentialInput, CreateUserInput, user as user_filter,
    user_credential as user_credential_filter,
};
use sms_api::{Principal, PrincipalKind};

use sms_api::system_context;

/// `Command::ProvisionUser`'s flags. See `Command::ProvisionUser`'s own
/// doc comment in `main.rs` — the enum variant carries the "why", this
/// struct only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct ProvisionUserArgs {
    #[arg(long, env = "DATABASE_URL")]
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

/// The actual `User` + `UserCredential` provisioning logic, shared by
/// [`provision_user_command`] and `bootstrap_command` — see
/// `seed_dispatch_core`'s (`commands::seed_dispatch`) own doc for why this
/// split exists.
///
/// Review round 1, item 12: all three writes (`User` create, `User`
/// update-subject, `UserCredential` create) now run in one
/// `run_in_isolated_tx` transaction, not three independent calls. Before
/// this, a failure between the `User` create and the `UserCredential`
/// create left a real, undetected orphan: the `User` row survived with
/// no password, and a later re-run hit `23505` on the email and reported
/// "already exists — skipping" — a clean exit 0 for an account nobody
/// could ever log into. [`check_existing_user_has_credential`] is the
/// other half of the fix: the duplicate-email path no longer trusts that
/// a matching `User` row means a *usable* one.
///
/// Returns `Ok(None)` rather than an error on a duplicate `email` (a
/// `23505` on `users_email_key`, narrowed by constraint name — item 13 —
/// so an unrelated `23505` propagates loudly instead of being folded
/// into "already exists") — the same "already-provisioned is success,
/// not failure" idiom every other seed/provision function in this file
/// uses, and specifically what `bootstrap_command` needs to stay a clean
/// no-op re-run against an already-bootstrapped deployment, *provided*
/// the existing account actually has a credential. The caller decides
/// what to print; there is no password to hand back for a `User` this
/// call didn't create.
pub(crate) async fn create_console_user_if_absent(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    email: &str,
    display_name: &str,
    role_key: &str,
) -> Result<Option<(String, String)>> {
    // #52/#58: both the password generator and the hasher now live in
    // `sms_core::password` — the console's own `provisionUser` procedure
    // needs the identical logic from `sms-api`, which cannot depend on
    // `sms-auth` (where this used to live). See that module's own doc.
    let password = sms_core::password::generate_password(24);
    let password_hash = sms_core::password::hash_password(&password)
        .map_err(|error| anyhow::anyhow!("hashing the generated password: {error}"))?;

    // `User.create`'s own `@@allow` is `hasRole('owner') ||
    // hasRole('admin')` — never `hasRole('system')` — so this needs its
    // own human-role-shaped context distinct from `sys`, the same split
    // `Command::ProvisionUser`'s original body already made.
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

    let created_user_id = cratestack::run_in_isolated_tx(
        db.pool(),
        cratestack::TransactionIsolation::Serializable,
        |mut tx| {
            let ctx = &ctx;
            let email = email_owned.clone();
            let display_name = display_name_owned.clone();
            let role_key = role_key_owned.clone();
            let password_hash = password_hash.clone();
            async move {
                let user = match db
                    .user()
                    .create(CreateUserInput {
                        // The OP is itself the identity source for a locally
                        // authenticated user (#194's own login.rs module doc —
                        // no external IdP is wired up), so `subject` is simply
                        // this row's own id, the same way `authenticate_user`'s
                        // Identity construction (backends/apps/sms-gateway/src/login.rs)
                        // uses `User.id` as `external_id`. `db.user().create`
                        // doesn't know its own generated id ahead of the call,
                        // so this writes a unique placeholder (subject is
                        // @unique — a fixed literal here would make a second
                        // concurrent run collide on it before either gets to
                        // the corrective update below) and immediately
                        // corrects it in a second update, in the same
                        // transaction now.
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

                // `UserCredential.create`'s own `@@allow` is
                // `hasRole('system')` only — never the owner-role `ctx`
                // the two `User` writes above use. Found live (review
                // round 1): the first cut of this transaction wrap
                // passed `ctx` here by mistake, which the pre-transaction
                // version never could have, since it always called
                // `.run(sys)` on a separately-typed value. `sys` is a
                // plain `&CratestackContext` (`Copy`), captured directly.
                db.user_credential()
                    .create(CreateUserCredentialInput {
                        userId: user.id.clone(),
                        passwordHash: password_hash,
                    })
                    .run_in_tx(&mut tx, sys)
                    .await?;

                Ok((Some(user.id), tx))
            }
        },
    )
    .await
    .context("creating the User row — check that --role-key names an existing Role")?;

    let Some(user_id) = created_user_id else {
        return check_existing_user_has_credential(db, sys, email).await;
    };

    Ok(Some((user_id, password)))
}

/// The duplicate-`email` path out of [`create_console_user_if_absent`] —
/// see that function's own doc for why this exists (review round 1, item
/// 12): a `23505` on `users_email_key` proves a matching `User` row
/// exists, not that it's usable. Looks the row up and confirms a
/// `UserCredential` actually exists for it before reporting "already
/// exists" as though nothing is wrong; if one doesn't, refuses loudly
/// and names what a human has to do next, since this command has no way
/// to repair an orphaned row automatically (there is no
/// `--force`/overwrite path for a security-sensitive credential write).
async fn check_existing_user_has_credential(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    email: &str,
) -> Result<Option<(String, String)>> {
    let existing = db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(user_filter::email().eq(email.to_owned())))
        .limit(1)
        .run(sys)
        .await
        .context("looking up the existing User row after a duplicate-email create")?
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "User row with email {email:?} reported as a duplicate on create but not \
                 found on lookup"
            )
        })?;

    let has_credential = !db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            user_credential_filter::userId().eq(existing.id.clone()),
        ))
        .limit(1)
        .run(sys)
        .await
        .context("checking for an existing UserCredential")?
        .is_empty();

    if !has_credential {
        bail!(
            "a User with email {email:?} already exists (id={}) but has no UserCredential — an \
             earlier provisioning attempt was interrupted after creating the User row but \
             before creating its credential, leaving an account nobody can log into. This \
             command cannot repair it automatically: delete the orphaned User row (a direct \
             generated-CRUD write under a bootstrapped owner session, or psql against \
             users/user_credentials) and re-run, or provision under a different --owner-email.",
            existing.id
        );
    }

    Ok(None)
}

/// `Command::ProvisionUser`'s body (#194) — see that variant's own doc
/// comment for what this does, why it exists, and why the password is
/// generated rather than accepted as a flag. Takes [`ProvisionUserArgs`]
/// directly rather than the whole `Command`: `main`'s own dispatch already
/// extracted it from `Command::ProvisionUser` at the match site.
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
    let sys = system_context("sms-gateway:op");

    let Some((user_id, password)) =
        create_console_user_if_absent(&db, &sys, &email, &display_name, &role_key).await?
    else {
        println!(
            "a User with email {email:?} already exists — nothing to do (no password to print; \
             it was only ever shown once, at that account's own provisioning time)"
        );
        return Ok(());
    };

    println!("provisioned user: {email} (id={user_id})");
    println!("one-time password (never stored, never shown again): {password}");
    println!();
    // #52/#58 landed the users-and-roles screens (`provisionUser` is the
    // console-side equivalent of this command), but still no
    // rotate/reset — see OPEN_QUESTIONS.md §3.6 for why, and what
    // deciding one for real would need.
    println!("no password-rotation flow exists yet — see OPEN_QUESTIONS.md §3.6 —");
    println!(
        "share this over a channel the recipient controls, not this command's own stdout log."
    );
    Ok(())
}
