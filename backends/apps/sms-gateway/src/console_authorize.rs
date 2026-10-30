#![doc = include_str!("console_authorize.md")]

use authkestra_op::handlers::authorize::AuthorizeRequest;
use serde::{Deserialize, Serialize};

/// The shortest and longest `code_challenge` the console's sign-in takes: RFC
/// 7636, section 4.2 (43 to 128 characters), and a `S256` challenge is always the
/// 43 of a base64url-encoded SHA-256.
const CODE_CHALLENGE_LENGTH: std::ops::RangeInclusive<usize> = 43..=128;

/// The console's authorization request: the eight OAuth parameters a sign-in
/// carries, on `POST /login`'s body and on `GET /login/sso/start`'s query alike.
///
/// `snake_case`, the OAuth parameter names themselves, pinned by a test. It is
/// also what an SSO transaction stores (`SsoTransaction.consoleRequest`) and
/// replays into `handle_authorize` after the upstream sign-in, so the request the
/// console made is the request the code is issued for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) struct ConsoleAuthorize {
    pub(crate) client_id: String,
    pub(crate) redirect_uri: String,
    pub(crate) response_type: String,
    pub(crate) scope: String,
    pub(crate) state: String,
    pub(crate) code_challenge: String,
    pub(crate) code_challenge_method: String,
    pub(crate) nonce: String,
}

impl ConsoleAuthorize {
    /// What the request alone proves wrong, before it costs a bucket token, an
    /// Argon2 hash or a database row: a challenge method other than `S256`, a
    /// challenge that is not 43 to 128 base64url characters (the library accepts
    /// an empty one), and a `redirect_uri` the console is not registered with.
    /// Each is a malformed *request*, so each is a `400` and never the person's
    /// `401`: nothing here depends on who the caller is.
    pub(crate) fn malformed(&self, redirect_uris: &[String]) -> Option<&'static str> {
        if self.code_challenge_method != "S256" {
            return Some("code_challenge_method");
        }
        if !CODE_CHALLENGE_LENGTH.contains(&self.code_challenge.len())
            || !self
                .code_challenge
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Some("code_challenge");
        }
        if !redirect_uris.contains(&self.redirect_uri) {
            return Some("redirect_uri");
        }
        None
    }

    /// The library's request for this one.
    ///
    /// `AuthorizeRequest` is `#[non_exhaustive]` (authkestra-op 0.8.0), so it cannot
    /// be built with a struct literal from outside the crate, and has no constructor;
    /// it derives `Deserialize`, so the wire shape is built by hand and deserialised.
    /// A future release that renames a field would not fail this for an `Option`
    /// field (it would silently become `None`): `the_authorize_request_keeps_its_optional_fields`
    /// below and the live suites' `state`/`nonce` round trips are what catch that.
    ///
    /// # Errors
    ///
    /// The JSON does not deserialise: a release of the library changed the shape.
    pub(crate) fn authorize_request(&self) -> Result<AuthorizeRequest, serde_json::Error> {
        serde_json::from_value(serde_json::json!({
            "client_id": self.client_id,
            "redirect_uri": self.redirect_uri,
            "response_type": self.response_type,
            "scope": self.scope,
            "state": self.state,
            "code_challenge": self.code_challenge,
            "code_challenge_method": self.code_challenge_method,
            "nonce": self.nonce,
        }))
    }
}

/// Whether `redirect` is the library's error redirect (`?error=...`) and not a
/// code.
///
/// The library answers a request it refused (a method other than `S256`, a scope
/// the client may not hold, a response type that is not `code`) with a redirect to
/// the client carrying `error=`. The console must never be handed that as though
/// it were a sign-in, so both routes map it to a `400` and forward nothing.
pub(crate) fn redirect_carries_an_error(redirect: &str) -> bool {
    match url::Url::parse(redirect) {
        Ok(url) => {
            let has = |name: &str| url.query_pairs().any(|(key, _)| key == name);
            has("error") || !has("code")
        }
        // Not a URL at all is certainly not a code.
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn request() -> ConsoleAuthorize {
        ConsoleAuthorize {
            client_id: "sms-console".to_owned(),
            redirect_uri: "https://admin.example/api/auth/callback".to_owned(),
            response_type: "code".to_owned(),
            scope: "openid profile".to_owned(),
            state: "state-abc123".to_owned(),
            code_challenge: "challenge-abc123".to_owned(),
            code_challenge_method: "S256".to_owned(),
            nonce: "nonce-abc123".to_owned(),
        }
    }

    /// The cheap, no-database half of the guard `authorize_request`'s doc names:
    /// the four `Option<String>` fields of `AuthorizeRequest` a rename would
    /// silently turn into `None` arrive `Some` for realistic input.
    #[test]
    fn the_authorize_request_keeps_its_optional_fields() {
        let authorize = request().authorize_request().expect("always deserializes");
        assert_eq!(authorize.state.as_deref(), Some("state-abc123"));
        assert_eq!(
            authorize.code_challenge.as_deref(),
            Some("challenge-abc123")
        );
        assert_eq!(authorize.code_challenge_method.as_deref(), Some("S256"));
        assert_eq!(authorize.nonce.as_deref(), Some("nonce-abc123"));
        assert_eq!(authorize.scope, "openid profile");
    }

    #[test]
    fn the_wire_shape_is_snake_case_and_refuses_unknown_fields() {
        let json = serde_json::to_value(request()).unwrap();
        assert_eq!(json["client_id"], "sms-console");
        assert!(serde_json::from_value::<ConsoleAuthorize>(json.clone()).is_ok());
        let mut camel = json.clone();
        camel["clientId"] = camel["client_id"].take();
        camel.as_object_mut().unwrap().remove("client_id");
        assert!(serde_json::from_value::<ConsoleAuthorize>(camel).is_err());
        let mut extra = json;
        extra["remember_me"] = true.into();
        assert!(serde_json::from_value::<ConsoleAuthorize>(extra).is_err());
    }

    fn registered() -> Vec<String> {
        vec!["https://admin.example/api/auth/callback".to_owned()]
    }

    /// A request that is well formed for the three cheap checks.
    fn well_formed() -> ConsoleAuthorize {
        ConsoleAuthorize {
            code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned(),
            redirect_uri: registered()[0].clone(),
            ..request()
        }
    }

    #[test]
    fn a_well_formed_request_passes_the_cheap_checks() {
        assert_eq!(well_formed().malformed(&registered()), None);
    }

    #[test]
    fn a_challenge_method_other_than_s256_is_malformed() {
        for method in ["plain", "s256", "", "S256 ", "S512"] {
            let request = ConsoleAuthorize {
                code_challenge_method: method.to_owned(),
                ..well_formed()
            };
            assert_eq!(
                request.malformed(&registered()),
                Some("code_challenge_method"),
                "{method:?}"
            );
        }
    }

    #[test]
    fn a_challenge_must_be_43_to_128_base64url_characters() {
        let with = |challenge: String| ConsoleAuthorize {
            code_challenge: challenge,
            ..well_formed()
        };
        for bad in [
            String::new(),
            "a".repeat(42),
            "a".repeat(129),
            format!("{}=", "a".repeat(43)),
            format!("{}+", "a".repeat(43)),
            format!("{}/", "a".repeat(43)),
            format!("{} ", "a".repeat(43)),
            format!("{}é", "a".repeat(42)),
        ] {
            assert_eq!(
                with(bad.clone()).malformed(&registered()),
                Some("code_challenge"),
                "{bad:?}"
            );
        }
        for good in ["a".repeat(43), "A-_9".repeat(32)] {
            assert_eq!(with(good.clone()).malformed(&registered()), None, "{good}");
        }
    }

    #[test]
    fn a_redirect_uri_that_is_not_registered_is_malformed_whatever_it_resembles() {
        for uri in [
            "https://admin.example/api/auth/callback/",
            "https://admin.example/api/auth/callback?x=1",
            "HTTPS://admin.example/api/auth/callback",
            "https://evil.example/api/auth/callback",
            "",
        ] {
            let request = ConsoleAuthorize {
                redirect_uri: uri.to_owned(),
                ..well_formed()
            };
            assert_eq!(
                request.malformed(&registered()),
                Some("redirect_uri"),
                "{uri:?}"
            );
        }
    }

    #[test]
    fn an_error_redirect_is_not_a_sign_in() {
        assert!(redirect_carries_an_error(
            "https://x/cb?error=invalid_request&error_description=nope&state=s"
        ));
        assert!(
            redirect_carries_an_error("https://x/cb?state=s"),
            "no code is no sign-in"
        );
        assert!(!redirect_carries_an_error("https://x/cb?code=abc&state=s"));
        assert!(redirect_carries_an_error("not a url"));
    }
}
