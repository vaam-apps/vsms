//! Persistence for the SSO broker: in-flight transactions and federated
//! links. Every read and write goes through a `CrateStack` delegate (R1); the
//! models are `hasRole('system')` only, so all of it runs under the
//! gateway's system context.

use chrono::{DateTime, Duration, Utc};
use cratestack::{CratestackContext, CratestackError, FilterExpr};
use sha2::{Digest, Sha256};
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

fn internal(what: &str, error: &CratestackError) -> SsoError {
    tracing::error!(%error, what, "SSO persistence failed");
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
        tracing::info!(%error, "SSO transaction already consumed");
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

/// A context that satisfies `User.create`'s policy (`owner` or `admin`).
/// Same precedent as `provision-user`; `admin` rather than `owner` because
/// it is all that policy needs.
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

/// Stamps `lastLoginAt`. Best effort: a lost optimistic-lock race or a
/// transient error here must not fail a sign-in that was already decided.
async fn touch_last_login(db: &Cratestack, sys: &CratestackContext, row: &schema::User) {
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
        tracing::warn!(%error, "could not stamp lastLoginAt after an SSO sign-in");
    }
}

/// Resolves a verified sign-in to a vsms account: the user already linked to
/// `(issuer, sub)`, or - on a first sign-in - a freshly provisioned one. The
/// caller has already applied the MFA and group rules.
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
            touch_last_login(db, sys, &user).await;
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

/// Creates the `User` and its link in one transaction. `Ok(None)` means the
/// link already exists (a concurrent first sign-in won).
async fn provision(
    db: &Cratestack,
    sys: &CratestackContext,
    config: &SsoConfig,
    claims: &IdClaims,
) -> Result<Option<ResolvedUser>, SsoError> {
    let email = claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| plausible_email(e))
        .ok_or(Refusal::EmailMissing)?
        .to_owned();
    let display_name = display_name_for(claims, &email);
    let creator = jit_context();
    let now = Utc::now();

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
                        lastLoginAt: Some(now),
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
        Err(error @ CratestackError::Validation(_)) => {
            tracing::warn!(%error, "the IdP's email or name did not validate as a User");
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
