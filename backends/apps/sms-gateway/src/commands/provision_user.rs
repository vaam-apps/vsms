//! `Command::ProvisionUser` (#194) — see that variant's own doc comment in
//! `main.rs` for what this does and why it exists. It makes a `User` row and,
//! only with `--password` (VSA-4a), a password written to a file; otherwise no
//! credential at all.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateUserCredentialInput, CreateUserInput, UpdateUserCredentialInput,
    user as user_filter, user_credential as user_credential_filter,
};
use sms_api::{Principal, PrincipalKind, system_context};
use sms_auth::gate::HumanLogin;

use crate::commands::secret_file::write_new_secret_file;

/// How long a generated password is. 24 characters is about 142 bits.
const PASSWORD_LEN: usize = 24;

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

    /// Also give the person a password (VSA-4a), generated here and written to
    /// `--password-out`, never printed. Needs a deployment that serves
    /// `human_login`; refused before the database is touched otherwise. An
    /// existing account that has none gets one; an existing password is refused
    /// unless `--reset`.
    #[arg(long = "password", requires = "credential_file")]
    pub(crate) with_password: bool,

    /// Where to write the generated password: a new file, `0600`, created with
    /// `O_EXCL` so an existing file is never overwritten. Read it once, hand it to
    /// the person, delete it.
    #[arg(long = "password-out", requires = "with_password")]
    pub(crate) credential_file: Option<PathBuf>,

    /// Replace the password of an account that already has one. Without it an
    /// existing password is refused: a reset is a decision, and a script that
    /// re-runs `provision-user` must not silently lock a person out.
    #[arg(long, requires = "with_password")]
    pub(crate) reset: bool,

    /// The YAML deployment configuration, read only for `--password`: it says
    /// whether this deployment serves `human_login`. Without `--password` this
    /// command reads no configuration.
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,
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

/// What [`issue_password`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasswordIssued {
    /// The account had no password; it has one now.
    Set,
    /// The account had a password and `--reset` replaced it.
    Replaced,
}

/// The `User.id` for `email`, if the row exists.
pub(crate) async fn user_id_by_email(db: &Cratestack, email: &str) -> Result<Option<String>> {
    Ok(db
        .user()
        .find_many()
        .where_expr(FilterExpr::from(user_filter::email().eq(email.to_owned())))
        .limit(1)
        .run(&system_context("sms-gateway:provision-user"))
        .await
        .context("looking up the User row by email")?
        .into_iter()
        .next()
        .map(|row| row.id))
}

/// Gives `user_id` a generated password and writes it to `out`.
///
/// - An existing password is refused unless `reset`, **before** a password is
///   generated or a file is made.
/// - The password is generated, hashed and written to `out` (`0600`, `O_EXCL`)
///   **before** the hash is stored, and the file is removed if storing fails: a
///   credential nobody holds the password to is the worse failure, and a file
///   with no credential is only a file. It is never printed and never in an error.
/// - `UserCredential` is `hasRole('system')` on every action, so the writes run
///   under the internal `system` context.
pub(crate) async fn issue_password(
    db: &Cratestack,
    user_id: &str,
    out: &Path,
    reset: bool,
) -> Result<PasswordIssued> {
    let sys = system_context("sms-gateway:provision-user");
    let existing = db
        .user_credential()
        .find_many()
        .where_expr(FilterExpr::from(
            user_credential_filter::userId().eq(user_id.to_owned()),
        ))
        .limit(1)
        .run(&sys)
        .await
        .context("checking for an existing UserCredential")?
        .into_iter()
        .next();
    if existing.is_some() && !reset {
        bail!(
            "this user already has a password; pass --reset to replace it (the old one stops \
             working at once)"
        );
    }

    let password = sms_core::password::generate_password(PASSWORD_LEN);
    let password_hash = sms_core::password::hash_password(&password)
        .map_err(|error| anyhow::anyhow!("hashing the generated password: {error}"))?;
    write_new_secret_file(out, &password, "password")?;

    let stored = match &existing {
        Some(credential) => db
            .user_credential()
            .update(credential.id.clone())
            .set(UpdateUserCredentialInput {
                passwordHash: Some(password_hash),
                ..Default::default()
            })
            .run(&sys)
            .await
            .map(drop),
        None => db
            .user_credential()
            .create(CreateUserCredentialInput {
                userId: user_id.to_owned(),
                passwordHash: password_hash,
            })
            .run(&sys)
            .await
            .map(drop),
    };
    if let Err(error) = stored {
        // Best effort: the credential was not stored, so the file holds a
        // password that opens nothing.
        let _ = std::fs::remove_file(out);
        return Err(error).context("storing the password hash");
    }
    Ok(if existing.is_some() {
        PasswordIssued::Replaced
    } else {
        PasswordIssued::Set
    })
}

/// What an operator is told after a password was written: where it is, and that
/// it is not shown.
pub(crate) fn announce_password(issued: PasswordIssued, email: &str, out: &Path) {
    let verb = match issued {
        PasswordIssued::Set => "set",
        PasswordIssued::Replaced => "replaced",
    };
    println!("password {verb} for {email}");
    println!(
        "written to: {} (0600, never printed): read it once, hand it to the person, delete it",
        out.display()
    );
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
        with_password,
        credential_file,
        reset,
        config,
    } = args;

    // Refused before the database is touched, in this order: the deployment must
    // serve the sign-in at all, and the file must not already be there.
    let credential_file = if with_password {
        let file = credential_file.context("--password needs --password-out")?;
        let loaded = config
            .load(sms_config::Binary::Operator)
            .context("loading the deployment configuration")?;
        HumanLogin::from_methods(&loaded.auth_methods).require()?;
        if file.exists() {
            bail!(
                "{} already exists - refusing to overwrite a file that may hold a password \
                 still in use; pass a different --password-out",
                file.display()
            );
        }
        Some(file)
    } else {
        None
    };

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    // The account, announced by its id alone and before the credential is
    // looked at: nothing printed below shares a variable with the password file.
    let created = create_console_user_if_absent(&db, &email, &display_name, &role_key).await?;
    announce_account(created.as_deref(), &email, credential_file.is_some());

    let Some(file) = credential_file else {
        return Ok(());
    };
    let user_id = match created {
        Some(user_id) => user_id,
        None => user_id_by_email(&db, &email)
            .await?
            .with_context(|| format!("the User with email {email:?} vanished"))?,
    };
    let issued = issue_password(&db, &user_id, &file, reset).await?;
    announce_password(issued, &email, &file);
    Ok(())
}

/// What an operator is told about the account itself: its id when it was just
/// made, and what to do about a credential when none was asked for. Never the
/// password file: it is handed no path.
fn announce_account(created: Option<&str>, email: &str, credential_asked_for: bool) {
    match (created, credential_asked_for) {
        (Some(user_id), false) => {
            println!("provisioned user: id={user_id}");
            println!(
                "no credential exists to hand over: pair a browser key with `sms-gateway \
                 pair-console-key --user {email} --public-key <key>`, or through a vaam \
                 admin phone; or re-run with --password --password-out <path> on a \
                 deployment that serves human_login."
            );
        }
        (Some(user_id), true) => println!("provisioned user: id={user_id}"),
        (None, false) => println!("a User with email {email:?} already exists — nothing to do"),
        (None, true) => {
            println!("a User with email {email:?} already exists — its role is not changed");
        }
    }
}
