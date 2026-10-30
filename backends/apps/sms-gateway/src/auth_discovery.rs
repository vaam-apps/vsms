#![doc = include_str!("auth_discovery.md")]

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use sms_config::AuthMethods;

/// Where a client learns which ways of authenticating this deployment serves.
pub const PATH: &str = "/.well-known/vsms-auth";

/// The names `auth.methods` spells them with, in `AuthMethod::ALL` order.
#[derive(Clone)]
struct Served(Vec<&'static str>);

/// `GET /.well-known/vsms-auth`: the methods this deployment **permits and
/// serves** (`AuthMethods::served`), resolved once at boot.
pub fn router(methods: &AuthMethods) -> Router {
    let served = Served(
        methods
            .served()
            .map(sms_config::AuthMethod::as_str)
            .collect(),
    );
    Router::new().route(PATH, get(discovery)).with_state(served)
}

async fn discovery(State(Served(methods)): State<Served>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "methods": methods }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use sms_config::{AuthConfig, AuthMethod};
    use tower::ServiceExt;

    /// `auth.methods` as written: `None` is the absent key.
    fn methods_of(written: Option<&[&str]>) -> AuthMethods {
        AuthConfig {
            methods: written.map(|names| names.iter().map(|name| (*name).to_owned()).collect()),
        }
        .resolve()
        .expect("a legal list")
    }

    async fn get_discovery(
        methods: &AuthMethods,
    ) -> (StatusCode, Option<String>, serde_json::Value) {
        let response = router(methods)
            .oneshot(Request::get(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        (
            status,
            content_type,
            serde_json::from_slice(&bytes).unwrap(),
        )
    }

    #[tokio::test]
    async fn an_absent_key_lists_what_the_release_serves_not_what_it_permits() {
        let (status, content_type, body) = get_discovery(&methods_of(None)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type.is_some_and(|ct| ct.starts_with("application/json")));
        assert_eq!(
            body,
            serde_json::json!({ "methods": ["keypair", "hmac_webhooks"] })
        );
    }

    /// `auth.methods` and its blocks as a deployment writes them: the repository's
    /// own base file with `overlay` on top, loaded the way `serve` loads it.
    fn methods_with_blocks(overlay: &str) -> AuthMethods {
        let config = sms_test_support::config::TestConfig::write(
            &std::env::temp_dir(),
            "auth-discovery",
            overlay,
        );
        let args = config.args();
        sms_config::Config::load_with_env(
            Some(std::path::Path::new(&args[1])),
            &args[3],
            sms_config::Binary::Operator,
            &|_| None,
        )
        .expect("a legal deployment")
        .auth_methods
    }

    const BLOCKS: &str = "op:\n  issuer: https://sms.example.org\n  token_rate_limit:\n    burst: 10\n    \
        refill_per_second: 0.3\n  signing_key:\n    refresh_interval_secs: 60\n    \
        rotation_overlap_secs: 1800\nhuman_login:\n  console_client:\n    client_id: sms-console\n    \
        redirect_uris:\n      - https://admin.example.org/cb\n  login_rate_limit:\n    burst: 5\n    \
        refill_per_second: 0.1\n";

    #[tokio::test]
    async fn human_login_is_listed_exactly_when_it_is_served() {
        let (_, _, body) = get_discovery(&methods_with_blocks(BLOCKS)).await;
        assert_eq!(
            body,
            serde_json::json!({
                "methods": ["keypair", "machine_tokens", "hmac_webhooks", "human_login"]
            })
        );
        // Permitted by the absent default, block not written: not listed.
        let (_, _, body) = get_discovery(&methods_of(None)).await;
        assert!(
            !body["methods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "human_login")
        );
        // Listed methods narrowed to the sign-in beside the keypair: no tokens.
        let narrowed = format!("auth:\n  methods: [keypair, human_login]\n{BLOCKS}");
        let (_, _, body) = get_discovery(&methods_with_blocks(&narrowed)).await;
        assert_eq!(
            body,
            serde_json::json!({ "methods": ["keypair", "human_login"] })
        );
    }

    #[tokio::test]
    async fn the_vaam_shape_lists_the_keypair_alone() {
        let (_, _, body) = get_discovery(&methods_of(Some(&["keypair"]))).await;
        assert_eq!(body, serde_json::json!({ "methods": ["keypair"] }));
    }

    #[tokio::test]
    async fn a_list_that_keeps_the_hmac_webhooks_says_so() {
        let (_, _, body) = get_discovery(&methods_of(Some(&["keypair", "hmac_webhooks"]))).await;
        assert_eq!(
            body,
            serde_json::json!({ "methods": ["keypair", "hmac_webhooks"] })
        );
    }

    #[tokio::test]
    async fn only_get_on_exactly_this_path_answers() {
        let app = router(&methods_of(None));
        let post = app
            .clone()
            .oneshot(Request::post(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        let other = app
            .oneshot(
                Request::get("/.well-known/vpay-auth")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::NOT_FOUND);
    }

    /// The discovery vocabulary is `AuthMethod`'s own.
    #[test]
    fn the_discovery_names_are_the_yaml_spellings() {
        let names: Vec<&str> = AuthMethod::ALL
            .into_iter()
            .map(AuthMethod::as_str)
            .collect();
        assert_eq!(
            names,
            ["keypair", "machine_tokens", "hmac_webhooks", "human_login"]
        );
    }
}
