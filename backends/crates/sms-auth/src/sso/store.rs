//! Persistence for the SSO broker: in-flight transactions and federated
//! links. Every read and write goes through a `CrateStack` delegate (R1); the
//! models are `hasRole('system')` only, so all of it runs under the
//! gateway's system context.

use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sha2::{Digest, Sha256};
use sms_api::auth::{DEVICE_ONLY_ROLES, RESERVED_ROLE_KEYS};
use sms_api::schema::{self, Cratestack, federated_link, sso_transaction, user};
use sms_api::{Principal, PrincipalKind};
use subtle::ConstantTimeEq;

use super::config::SsoConfig;
use super::error::{Refusal, SsoError};
use super::id_token::IdClaims;
use super::provider::AuthnSecrets;

/// How long an upstream sign-in may take before its transaction expires.
/// Equal to the console's own `vsms_oidc_txn` cookie lifetime: past it the
/// console could not complete the exchange anyway.
pub const TRANSACTION_TTL: Duration = Duration::minutes(5);

const UNIQUE_VIOLATION: &str = sms_api::errors::UNIQUE_VIOLATION;
const LINK_UNIQUE_CONSTRAINT: &str = "federated_links_issuer_subject_key";
const USER_EMAIL_CONSTRAINT: &str = "users_email_key";
const MAX_DISPLAY_NAME_CHARS: usize = 128;
const MAX_EMAIL_CHARS: usize = 254;

/// What `/login/sso/callback` gets back for a `state`.
#[derive(Debug, Clone)]
pub struct Transaction {
    /// The console's authorize request, as the JSON `AuthorizeRequest`
    /// deserializes from.
    pub console_request: String,
    /// The `nonce` sent upstream.
    pub nonce: String,
    /// The PKCE verifier for the upstream code exchange.
    pub code_verifier: String,
    /// True for the single `prompt=login` retry.
    pub is_retry: bool,
}

/// The SHA-256 of `state`, hex. This - never `state` itself - is what the
/// table holds: a copy of the database cannot be replayed into a callback.
#[must_use]
pub fn state_hash(state: &str) -> String {
    hex::encode(Sha256::digest(state.as_bytes()))
}

/// What a log line may say about a database error: the SQLSTATE and the
/// constraint's name, and nothing else. A `CratestackError`'s `Display` can carry
/// the value that failed (an e-mail address the identity provider sent, a
/// `state` hash, a key's value), and the log is not where a person's address
/// belongs.
fn db_kind(error: &CratestackError) -> (&str, &str) {
    (
        error.db_sqlstate().unwrap_or("none"),
        error.db_constraint().unwrap_or("none"),
    )
}

fn internal(what: &str, error: &CratestackError) -> SsoError {
    let (sqlstate, constraint) = db_kind(error);
    tracing::error!(what, sqlstate, constraint, "SSO persistence failed");
    SsoError::Unavailable(format!("{what} failed"))
}

/// Records a new transaction.
///
/// # Errors
///
/// [`SsoError::Unavailable`] when the write fails.
pub async fn begin_transaction(
    db: &Cratestack,
    sys: &CratestackContext,
    secrets: &AuthnSecrets,
    console_request: &str,
    is_retry: bool,
) -> Result<(), SsoError> {
    db.sso_transaction()
        .create(schema::CreateSsoTransactionInput {
            stateHash: state_hash(&secrets.state),
            nonce: secrets.nonce.clone(),
            codeVerifier: secrets.code_verifier.clone(),
            consoleRequest: console_request.to_owned(),
            isRetry: is_retry,
            expiresAt: Utc::now() + TRANSACTION_TTL,
        })
        .run(sys)
        .await
        .map_err(|e| internal("recording the SSO transaction", &e))?;
    Ok(())
}

/// Consumes the transaction for `state`: single use (the row is deleted, and
/// a second concurrent delete of the same row affects nothing), and expired
/// rows are consumed too but reported as absent.
///
/// `Ok(None)` is every "no such sign-in in flight": unknown, replayed,
/// expired. The caller cannot and need not tell them apart.
///
/// # Errors
///
/// [`SsoError::Unavailable`] when the lookup itself fails.
pub async fn consume_transaction(
    db: &Cratestack,
    sys: &CratestackContext,
    state: &str,
) -> Result<Option<Transaction>, SsoError> {
    let wanted = state_hash(state);
    let row = db
        .sso_transaction()
        .find_many()
        .where_expr(FilterExpr::from(
            sso_transaction::stateHash().eq(wanted.clone()),
        ))
        .limit(1)
        .run(sys)
        .await
        .map_err(|e| internal("looking up the SSO transaction", &e))?
        .into_iter()
        .next();
    let Some(row) = row else {
        return Ok(None);
    };
    // The database matched on equality already; this is the explicit,
    // constant-time comparison the design calls for, kept so the property
    // does not depend on how Postgres happens to compare.
    if !bool::from(row.stateHash.as_bytes().ct_eq(wanted.as_bytes())) {
        return Ok(None);
    }
    // The delete is the gate. Of two concurrent callbacks for one `state`,
    // exactly one deletes the row; the other's delete finds nothing.
    if let Err(error) = db.sso_transaction().delete(row.id.clone()).run(sys).await {
        let (sqlstate, constraint) = db_kind(&error);
        tracing::info!(sqlstate, constraint, "SSO transaction already consumed");
        return Ok(None);
    }
    if row.expiresAt <= Utc::now() {
        return Ok(None);
    }
    Ok(Some(Transaction {
        console_request: row.consoleRequest,
        nonce: row.nonce,
        code_verifier: row.codeVerifier,
        is_retry: row.isRetry,
    }))
}

/// Deletes up to `limit` transactions that expired before `now`.
///
/// # Errors
///
/// [`SsoError::Unavailable`] when the lookup fails; a failed individual
/// delete (another replica got there first) is not an error.
pub async fn reap_expired(
    db: &Cratestack,
    sys: &CratestackContext,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<usize, SsoError> {
    let expired = db
        .sso_transaction()
        .find_many()
        .where_expr(FilterExpr::from(sso_transaction::expiresAt().lt(now)))
        .order_by(sso_transaction::expiresAt().asc())
        .limit(limit)
        .run(sys)
        .await
        .map_err(|e| internal("listing expired SSO transactions", &e))?;
    let mut reaped = 0;
    for row in expired {
        if db.sso_transaction().delete(row.id).run(sys).await.is_ok() {
            reaped += 1;
        }
    }
    Ok(reaped)
}

/// The vsms account a verified sign-in resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUser {
    /// `User.id` - becomes the OP token's `sub`.
    pub user_id: String,
    /// `User.email`.
    pub email: String,
    /// `User.displayName`.
    pub display_name: String,
    /// True when this sign-in created the account.
    pub provisioned: bool,
}

/// A context that satisfies `User.create`'s policy (`owner`, or `admin` creating a
/// role that is not `owner`). `admin` rather than `owner` because it is all that
/// policy needs - and it means the schema alone would let this creator create an
/// `admin`, which is why [`refuse_unusable_jit_role`] and the loader both refuse
/// one first. No `system` clause on `User.create` was added instead, on purpose:
/// it would widen a policy to save a synthetic principal.
fn jit_context() -> CratestackContext {
    Principal {
        sub: "sms-gateway:sso-jit".to_owned(),
        kind: PrincipalKind::User,
        role: "admin".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn display_name_for(claims: &IdClaims, email: &str) -> String {
    let raw = claims
        .name
        .as_deref()
        .or(claims.preferred_username.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| email.split('@').next().unwrap_or("SSO user"));
    let clipped: String = raw.chars().take(MAX_DISPLAY_NAME_CHARS).collect();
    if clipped.is_empty() {
        "SSO user".to_owned()
    } else {
        clipped
    }
}

/// Whether `email` is at the `.invalid` top-level domain (RFC 6761), which
/// console pairing writes for the accounts it creates (`<id>@vaam.invalid`).
fn is_reserved_domain(email: &str) -> bool {
    email
        .rsplit_once('@')
        .map(|(_, domain)| domain.trim_end_matches('.').to_ascii_lowercase())
        .is_some_and(|domain| domain == "invalid" || domain.ends_with(".invalid"))
}

/// Refuses to create an account under a role that must never be given to an
/// unsupervised sign-in.
///
/// The configuration loader already refuses it (`human_login.sso.jit_role`), and
/// the database refuses an `owner` from an `admin` creator. This is the third,
/// independent line, and the only one that reads the authoritative lists
/// (`sms_api::auth::RESERVED_ROLE_KEYS` and `DEVICE_ONLY_ROLES`) rather than a
/// copy: the JIT account is created under a synthetic `admin`, which the schema's
/// `User.create` policy lets create every role but `owner`. It is the operator's
/// error and not the person's, so it is `Unavailable` and not a refusal.
fn refuse_unusable_jit_role(role: &str) -> Result<(), SsoError> {
    if RESERVED_ROLE_KEYS.contains(&role) || DEVICE_ONLY_ROLES.contains(&role) {
        tracing::error!(
            role,
            "human_login.sso.jit_role names a role no sign-in may create; refusing to provision"
        );
        return Err(SsoError::Unavailable(
            "the configured just-in-time role is not allowed".to_owned(),
        ));
    }
    Ok(())
}

fn plausible_email(email: &str) -> bool {
    email.len() <= MAX_EMAIL_CHARS
        && !email.chars().any(char::is_whitespace)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
}

async fn find_link(
    db: &Cratestack,
    sys: &CratestackContext,
    issuer: &str,
    subject: &str,
) -> Result<Option<schema::FederatedLink>, SsoError> {
    Ok(db
        .federated_link()
        .find_many()
        .where_expr(
            FilterExpr::from(federated_link::issuer().eq(issuer.to_owned()))
                .and(federated_link::subject().eq(subject.to_owned())),
        )
        .limit(1)
        .run(sys)
        .await
        .map_err(|e| internal("looking up the federated link", &e))?
        .into_iter()
        .next())
}

/// The active, non-deleted `User` with this id, if any. `@@soft_delete`
/// already hides deleted rows from `find_many`.
async fn find_active_user(
    db: &Cratestack,
    sys: &CratestackContext,
    user_id: &str,
) -> Result<Option<schema::User>, SsoError> {
    Ok(db
        .user()
        .find_many()
        .where_expr(
            FilterExpr::from(user::id().eq(user_id.to_owned())).and(user::active().is_true()),
        )
        .limit(1)
        .run(sys)
        .await
        .map_err(|e| internal("looking up the user", &e))?
        .into_iter()
        .next())
}

/// Whether a `Role` row keyed `role` exists. `serve` asks at boot, once, so a
/// `human_login.sso.jit_role` that names nothing stops the gateway starting and not
/// the first person to sign in (whose account the foreign key would then refuse,
/// as an outage nobody could name).
///
/// # Errors
///
/// [`SsoError::Unavailable`] when the lookup itself fails.
pub async fn jit_role_exists(
    db: &Cratestack,
    sys: &CratestackContext,
    role: &str,
) -> Result<bool, SsoError> {
    Ok(!db
        .role()
        .find_many()
        .where_expr(FilterExpr::from(schema::role::key().eq(role.to_owned())))
        .limit(1)
        .run(sys)
        .await
        .map_err(|e| internal("looking up the just-in-time role", &e))?
        .is_empty())
}

/// Stamps `lastLoginAt` on `user_id`, **after** the sign-in it records has
/// issued its authorization code - never before, so a refused sign-in leaves no
/// trace on the account. Best effort: a lost optimistic-lock race or a transient
/// error here must not fail a sign-in that was already decided.
///
/// A first sign-in creates the account with no `lastLoginAt` for the same
/// reason: until a code is issued, nothing has been signed in.
pub async fn stamp_last_login(db: &Cratestack, sys: &CratestackContext, user_id: &str) {
    let row = match find_active_user(db, sys, user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(error) => {
            // `error` is an `SsoError`: a fixed description, never a value.
            tracing::warn!(%error, "could not read the account to stamp lastLoginAt");
            return;
        }
    };
    let result = db
        .user()
        .update(row.id.clone())
        .set(schema::UpdateUserInput {
            lastLoginAt: Some(Some(Utc::now())),
            ..Default::default()
        })
        .if_match(row.version)
        .run(sys)
        .await;
    if let Err(error) = result {
        let (sqlstate, constraint) = db_kind(&error);
        tracing::warn!(
            sqlstate,
            constraint,
            "could not stamp lastLoginAt after an SSO sign-in"
        );
    }
}

/// Resolves a verified sign-in to a vsms account: the user already linked to
/// `(issuer, sub)`, or - on a first sign-in - a freshly provisioned one. The
/// caller has already applied the MFA and group rules. It stamps nothing: the
/// caller calls [`stamp_last_login`] once a code has been issued.
///
/// # Errors
///
/// [`Refusal::UserInactive`] for a linked account that is deactivated or
/// deleted; [`Refusal::EmailMissing`] / [`Refusal::EmailInUse`] when a first
/// sign-in cannot create the account; [`SsoError::Unavailable`] on database
/// faults.
pub async fn resolve_user(
    db: &Cratestack,
    sys: &CratestackContext,
    config: &SsoConfig,
    claims: &IdClaims,
) -> Result<ResolvedUser, SsoError> {
    for _ in 0..2 {
        if let Some(link) = find_link(db, sys, &config.issuer, &claims.subject).await? {
            let user = find_active_user(db, sys, &link.userId)
                .await?
                .ok_or(Refusal::UserInactive)?;
            return Ok(ResolvedUser {
                user_id: user.id,
                email: user.email,
                display_name: user.displayName,
                provisioned: false,
            });
        }
        // `None`: lost the race to a concurrent first sign-in of this very
        // identity - its link exists now, so loop and read it.
        if let Some(resolved) = provision(db, sys, config, claims).await? {
            return Ok(resolved);
        }
    }
    Err(SsoError::Unavailable(
        "the federated link kept appearing and disappearing".to_owned(),
    ))
}

/// How many users one page of [`email_held_locally`] reads, and the most pages it
/// will read before it gives up.
const EMAIL_SCAN_PAGE: i64 = 500;
const EMAIL_SCAN_MAX_PAGES: i64 = 100;

/// Whether any live `User` holds `email`, **ignoring case**.
///
/// The unique index is on the exact text and the query builder has no
/// case-insensitive comparison (raw SQL is R1's to refuse), so this reads the
/// users a page at a time and compares in Rust. That is acceptable only because it
/// runs for a *first* sign-in of an identity, never for a returning one, and a
/// console's user table is staff. A table too large to scan (more than
/// `EMAIL_SCAN_PAGE * EMAIL_SCAN_MAX_PAGES` users) fails closed as an operator
/// problem and is a reason to add a normalised column, not to skip the check.
async fn email_held_locally(
    db: &Cratestack,
    sys: &CratestackContext,
    email: &str,
) -> Result<bool, SsoError> {
    let wanted = email.to_lowercase();
    for page in 0..EMAIL_SCAN_MAX_PAGES {
        let users = db
            .user()
            .find_many()
            .order_by(user::id().asc())
            .limit(EMAIL_SCAN_PAGE)
            .offset(page * EMAIL_SCAN_PAGE)
            .run(sys)
            .await
            .map_err(|e| internal("scanning the users for the e-mail", &e))?;
        if users.iter().any(|row| row.email.to_lowercase() == wanted) {
            return Ok(true);
        }
        if i64::try_from(users.len()).unwrap_or(i64::MAX) < EMAIL_SCAN_PAGE {
            return Ok(false);
        }
    }
    tracing::error!("the user table is too large to scan for a case-insensitive e-mail clash");
    Err(SsoError::Unavailable(
        "the user table is too large to check the e-mail".to_owned(),
    ))
}

/// Whether `email` can be given to a new account. `Ok(false)` is "a concurrent
/// first sign-in of this very identity created it a moment ago": the caller reads
/// the link. A genuine clash is `Refusal::EmailInUse`.
///
/// `users_email_key` is case-sensitive, so `Alice@corp.test` would sail past a
/// local `alice@corp.test` and create a second account for one mailbox: the check
/// ignores case ([`email_held_locally`]).
async fn email_is_free(
    db: &Cratestack,
    sys: &CratestackContext,
    config: &SsoConfig,
    claims: &IdClaims,
    email: &str,
) -> Result<bool, SsoError> {
    if !email_held_locally(db, sys, email).await? {
        return Ok(true);
    }
    if find_link(db, sys, &config.issuer, &claims.subject)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    Err(Refusal::EmailInUse.into())
}

/// Creates the `User` and its link in one transaction. `Ok(None)` means the
/// link already exists (a concurrent first sign-in won).
async fn provision(
    db: &Cratestack,
    sys: &CratestackContext,
    config: &SsoConfig,
    claims: &IdClaims,
) -> Result<Option<ResolvedUser>, SsoError> {
    refuse_unusable_jit_role(&config.jit_role_key)?;
    let email = claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| plausible_email(e))
        .ok_or(Refusal::EmailMissing)?
        .to_owned();
    // Only a sign-in that would CREATE an account is asked for a confirmed
    // address: a returning person is found by `(iss, sub)` and their email is
    // never read.
    if claims.email_verified != Some(true) {
        return Err(Refusal::EmailUnverified.into());
    }
    if is_reserved_domain(&email) {
        return Err(Refusal::EmailReserved.into());
    }
    if !email_is_free(db, sys, config, claims, &email).await? {
        return Ok(None);
    }
    let display_name = display_name_for(claims, &email);
    let creator = jit_context();
    let outcome = cratestack::run_in_isolated_tx(
        db.pool(),
        cratestack::TransactionIsolation::Serializable,
        |mut tx| {
            let creator = &creator;
            let email = email.clone();
            let display_name = display_name.clone();
            let role_key = config.jit_role_key.clone();
            let issuer = config.issuer.clone();
            let subject = claims.subject.clone();
            async move {
                let user = db
                    .user()
                    .create(schema::CreateUserInput {
                        // `subject` is vsms's own OP subject: the row's own
                        // id, exactly as `provision-user` writes it. It is
                        // NOT the upstream `sub` (that lives in the link).
                        // The id is not known before the insert, so a unique
                        // placeholder is written and corrected in this same
                        // transaction.
                        subject: format!("pending-{}", cratestack::uuid::Uuid::new_v4()),
                        email: email.clone(),
                        displayName: display_name.clone(),
                        roleKey: role_key,
                        lastLoginAt: None,
                        deletedAt: None,
                    })
                    .run_in_tx(&mut tx, creator)
                    .await?
                    .value;
                db.user()
                    .update(user.id.clone())
                    .set(schema::UpdateUserInput {
                        subject: Some(user.id.clone()),
                        ..Default::default()
                    })
                    .if_match(user.version)
                    .run_in_tx(&mut tx, creator)
                    .await?;
                db.federated_link()
                    .create(schema::CreateFederatedLinkInput {
                        issuer,
                        subject,
                        userId: user.id.clone(),
                    })
                    .run_in_tx(&mut tx, sys)
                    .await?;
                Ok((user.id, tx))
            }
        },
    )
    .await;

    match outcome {
        Ok(user_id) => Ok(Some(ResolvedUser {
            user_id,
            email,
            display_name,
            provisioned: true,
        })),
        Err(error) if error.db_sqlstate() == Some(UNIQUE_VIOLATION) => {
            match error.db_constraint() {
                Some(LINK_UNIQUE_CONSTRAINT) => Ok(None),
                Some(USER_EMAIL_CONSTRAINT) => {
                    // Either a local account really holds this email, or
                    // a concurrent first sign-in of this *same* identity
                    // created the user a moment ago (its INSERT reached
                    // `users_email_key` before ours reached the link's
                    // unique index). If the link exists now, it was the
                    // second; go read it. Otherwise it is a genuine clash.
                    if find_link(db, sys, &config.issuer, &claims.subject)
                        .await?
                        .is_some()
                    {
                        Ok(None)
                    } else {
                        Err(Refusal::EmailInUse.into())
                    }
                }
                _ => Err(internal("provisioning the SSO user", &error)),
            }
        }
        Err(CratestackError::Validation(_)) => {
            // Fixed text: the validation message names the value that failed, and
            // that is the identity provider's address or name.
            tracing::warn!("the IdP's email or name did not validate as a User");
            Err(Refusal::EmailMissing.into())
        }
        Err(error) => Err(internal("provisioning the SSO user", &error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_hash_is_a_stable_64_char_hex_digest_that_hides_the_state() {
        let hash = state_hash("some-state");
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, state_hash("some-state"));
        assert_ne!(hash, state_hash("some-state2"));
        assert!(!hash.contains("some-state"));
    }

    #[test]
    fn display_names_prefer_name_then_username_then_the_email_local_part() {
        let mut claims = IdClaims {
            name: Some("  Ada Lovelace ".to_owned()),
            preferred_username: Some("ada".to_owned()),
            ..IdClaims::default()
        };
        assert_eq!(display_name_for(&claims, "ada@x.test"), "Ada Lovelace");
        claims.name = None;
        assert_eq!(display_name_for(&claims, "ada@x.test"), "ada");
        claims.preferred_username = None;
        assert_eq!(display_name_for(&claims, "ada.l@x.test"), "ada.l");
    }

    #[test]
    fn display_names_are_clipped_to_the_column_limit() {
        let claims = IdClaims {
            name: Some("x".repeat(500)),
            ..IdClaims::default()
        };
        assert_eq!(display_name_for(&claims, "a@b.test").chars().count(), 128);
    }

    #[test]
    fn the_invalid_top_level_domain_is_reserved_in_any_spelling() {
        for reserved in [
            "abc@vaam.invalid",
            "abc@VAAM.INVALID",
            "abc@invalid",
            "abc@a.b.invalid.",
        ] {
            assert!(is_reserved_domain(reserved), "{reserved}");
        }
        for fine in ["a@b.test", "a@invalid.example", "a@notinvalid"] {
            assert!(!is_reserved_domain(fine), "{fine}");
        }
    }

    #[test]
    fn a_jit_role_that_a_sign_in_must_never_create_is_an_operator_error() {
        for role in ["system", "app", "owner", "admin"] {
            assert!(
                matches!(
                    refuse_unusable_jit_role(role),
                    Err(SsoError::Unavailable(_))
                ),
                "{role}"
            );
        }
        assert!(refuse_unusable_jit_role("support").is_ok());
    }

    #[test]
    fn the_configuration_copy_of_the_refused_roles_is_the_runtime_lists_union() {
        // sms-config cannot import these; this is the pin that keeps its copy honest.
        let mut union: Vec<&str> = RESERVED_ROLE_KEYS
            .iter()
            .chain(DEVICE_ONLY_ROLES)
            .copied()
            .collect();
        union.sort_unstable();
        let mut copy = sms_config::REFUSED_JIT_ROLES.to_vec();
        copy.sort_unstable();
        assert_eq!(copy, union);
    }

    #[test]
    fn implausible_emails_are_not_used_to_create_accounts() {
        assert!(plausible_email("a@b.test"));
        for bad in [
            "",
            "nobody",
            "a@b",
            "@b.test",
            "a b@c.test",
            &format!("{}@b.test", "x".repeat(300)),
        ] {
            assert!(!plausible_email(bad), "{bad:?}");
        }
    }
}
