//! The two conditions a verified `id_token` must meet before anyone is
//! linked or created: a multi-factor claim, and membership of the required
//! group. Pure functions of the claims and the configuration.
//!
//! **The MFA rule** - an `id_token` is accepted only if its `acr` is in
//! [`SsoConfig::accepted_acr`] **or** its `amr` shares a member with
//! [`SsoConfig::accepted_amr`]. An empty list matches nothing (never
//! "anything"): a token with no `amr` against an `amr`-only rule is refused.
//! Both lists empty is a configuration error the loader refuses
//! (`sms-config`, `human_login.sso.accepted_amr`), so this module never sees it.
//!
//! Measured against Keycloak 26.6.1, because the obvious reading is wrong:
//! `acr` `"1"` (password) and `"0"` (a repeat sign-in from the SSO cookie)
//! prove *nothing* about a second factor, and with the stock browser flow
//! `amr` is **empty** until the flow's executions are given an authenticator
//! config with a reference value. A deployment therefore accepts
//! `amr = ["otp"]` and leaves `acr` empty; it is deliberate that `acr "1"`
//! with `amr ["pwd"]` is refused.

use super::config::SsoConfig;
use super::error::Refusal;
use super::id_token::IdClaims;

/// What the callback should do with a verified token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Link or provision, then sign in.
    Admit,
    /// The only thing wrong is the MFA claim and this was the first attempt:
    /// send the user back upstream once with `prompt=login`.
    RetryWithPromptLogin,
    /// Refuse, with the reason for the log.
    Refuse(Refusal),
}

/// True when the token satisfies the MFA rule.
#[must_use]
pub fn mfa_satisfied(claims: &IdClaims, config: &SsoConfig) -> bool {
    let class_ok = claims
        .acr
        .as_ref()
        .is_some_and(|acr| config.accepted_acr.iter().any(|accepted| accepted == acr));
    let method_ok = claims
        .amr
        .iter()
        .any(|amr| config.accepted_amr.iter().any(|accepted| accepted == amr));
    class_ok || method_ok
}

/// True when `claims.groups` contains the required group (exact match).
#[must_use]
pub fn in_required_group(claims: &IdClaims, config: &SsoConfig) -> bool {
    claims.groups.contains(&config.required_group)
}

/// Decides what to do with a verified token.
///
/// The group gate is evaluated first and refuses outright. Only when the
/// group is fine and **the MFA claim alone** is missing does the first
/// attempt earn a retry: Keycloak enrols a new user's OTP through a required
/// action (so the first token has `amr ["pwd"]`) and then keeps issuing
/// `amr ["pwd"]` from the SSO cookie; a fresh authentication with
/// `prompt=login` re-runs the forms and yields `otp`. A retry that still
/// lacks the claim is refused - at most one retry, never a loop.
#[must_use]
pub fn evaluate(claims: &IdClaims, config: &SsoConfig, is_retry: bool) -> Verdict {
    if !in_required_group(claims, config) {
        return Verdict::Refuse(Refusal::NotInRequiredGroup);
    }
    if mfa_satisfied(claims, config) {
        return Verdict::Admit;
    }
    if is_retry {
        Verdict::Refuse(Refusal::MfaNotSatisfied)
    } else {
        Verdict::RetryWithPromptLogin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sso::config::tests::sso;

    fn config(acr: &[&str], amr: &[&str]) -> SsoConfig {
        SsoConfig::new(
            &sso(&serde_json::json!({ "accepted_acr": acr, "accepted_amr": amr })),
            "https://vsms.example.test",
            false,
        )
        .unwrap()
    }

    fn claims(acr: Option<&str>, amr: &[&str], groups: &[&str]) -> IdClaims {
        IdClaims {
            subject: "s".to_owned(),
            acr: acr.map(str::to_owned),
            amr: amr.iter().map(|s| (*s).to_owned()).collect(),
            groups: groups.iter().map(|s| (*s).to_owned()).collect(),
            ..IdClaims::default()
        }
    }

    #[test]
    fn an_accepted_amr_satisfies_the_rule() {
        let cfg = config(&[], &["otp"]);
        assert!(mfa_satisfied(&claims(None, &["pwd", "otp"], &[]), &cfg));
    }

    #[test]
    fn an_accepted_acr_satisfies_the_rule() {
        let cfg = config(&["urn:mfa"], &[]);
        assert!(mfa_satisfied(&claims(Some("urn:mfa"), &[], &[]), &cfg));
    }

    #[test]
    fn acr_one_with_amr_pwd_is_refused_against_an_amr_otp_rule() {
        let cfg = config(&[], &["otp"]);
        assert!(!mfa_satisfied(&claims(Some("1"), &["pwd"], &[]), &cfg));
    }

    #[test]
    fn a_token_with_no_amr_and_no_acr_never_satisfies_a_rule() {
        let cfg = config(&["urn:mfa"], &["otp"]);
        assert!(!mfa_satisfied(&claims(None, &[], &[]), &cfg));
    }

    #[test]
    fn the_group_must_match_exactly() {
        let cfg = config(&[], &["otp"]);
        assert!(in_required_group(
            &claims(None, &[], &["x", "vsms-staff"]),
            &cfg
        ));
        assert!(!in_required_group(
            &claims(None, &[], &["/vsms-staff", "vsms-staff2"]),
            &cfg
        ));
    }

    #[test]
    fn a_missing_group_refuses_even_when_mfa_is_also_missing() {
        let cfg = config(&[], &["otp"]);
        assert_eq!(
            evaluate(&claims(None, &["pwd"], &[]), &cfg, false),
            Verdict::Refuse(Refusal::NotInRequiredGroup)
        );
    }

    #[test]
    fn only_mfa_missing_on_the_first_attempt_earns_one_retry() {
        let cfg = config(&[], &["otp"]);
        let c = claims(None, &["pwd"], &["vsms-staff"]);
        assert_eq!(evaluate(&c, &cfg, false), Verdict::RetryWithPromptLogin);
        assert_eq!(
            evaluate(&c, &cfg, true),
            Verdict::Refuse(Refusal::MfaNotSatisfied)
        );
    }

    #[test]
    fn a_fully_satisfying_token_is_admitted_first_try_or_retry() {
        let cfg = config(&[], &["otp"]);
        let c = claims(None, &["pwd", "otp"], &["vsms-staff"]);
        assert_eq!(evaluate(&c, &cfg, false), Verdict::Admit);
        assert_eq!(evaluate(&c, &cfg, true), Verdict::Admit);
    }
}
