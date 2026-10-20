//! The callback's orchestration: from a returned `code` to an account.
//! Kept out of the gateway's axum handlers so a live-Postgres test can drive
//! it without HTTP, and so the order of the checks lives in one place.

use cratestack::CratestackContext;
use sms_api::schema::Cratestack;

use super::error::SsoError;
use super::policy::{self, Verdict};
use super::provider::SsoProvider;
use super::store::{self, ResolvedUser, Transaction};

/// What a successful run of the callback decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completed {
    /// Sign this account in.
    SignedIn(ResolvedUser),
    /// Send the browser back upstream once with `prompt=login`: the token
    /// was fine except it lacked the MFA claim, and this was the first try.
    RetryWithPromptLogin,
}

/// Runs the checks, in order, for one returned `code`:
///
/// 1. exchange the code (`client_secret_basic` + PKCE verifier);
/// 2. verify the `id_token` (algorithm, key, signature, `iss`, `aud`, `azp`,
///    `exp`/`iat`, `nonce`);
/// 3. the group gate, then the MFA rule ([`policy::evaluate`]);
/// 4. link, or provision just in time.
///
/// The first failure wins and nothing later runs - in particular no account
/// is created for a token that failed any earlier check.
///
/// # Errors
///
/// [`SsoError::Refused`] with the precise [`Refusal`]; [`SsoError::Unavailable`]
/// when the `IdP` or the database could not be used.
pub async fn complete(
    provider: &SsoProvider,
    db: &Cratestack,
    sys: &CratestackContext,
    transaction: &Transaction,
    code: &str,
) -> Result<Completed, SsoError> {
    let raw = provider
        .exchange_code(code, &transaction.code_verifier)
        .await?;
    let claims = provider.verify_id_token(&raw, &transaction.nonce).await?;
    match policy::evaluate(&claims, provider.config(), transaction.is_retry) {
        Verdict::Refuse(refusal) => Err(refusal.into()),
        Verdict::RetryWithPromptLogin => Ok(Completed::RetryWithPromptLogin),
        Verdict::Admit => {
            let resolved = store::resolve_user(db, sys, provider.config(), &claims).await?;
            Ok(Completed::SignedIn(resolved))
        }
    }
}
