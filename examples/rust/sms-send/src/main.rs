//! vsms integration example (Rust): the full HTTP path a third-party
//! backend uses to send one message through vsms — no in-process
//! shortcuts, no `Procedures::send_message` call, nothing this crate
//! could not also do from a different repository entirely.
//!
//! 1. Read the PEM `sms-gateway provision-client` wrote.
//! 2. Sign an RFC 7523 §3 `private_key_jwt` client assertion.
//! 3. Exchange it at `POST {issuer}/token` for a `client_credentials`
//!    access token.
//! 4. Call `POST {issuer}/$procs/sendMessage` with that Bearer token.
//! 5. Read the message back with `GET {issuer}/messages/{id}` and print
//!    its state — proving the write actually landed, not just that the
//!    mutation's own response claimed success.
//!
//! See `packages/gateway/src/token.ts` in the main vsms repo for the
//! canonical implementation of steps 1-3 (the admin console's own token
//! acquisition) — this mirrors it deliberately rather than inventing a
//! second interpretation of the same exchange.
//!
//! ```bash
//! cargo run -- \
//!   --issuer http://127.0.0.1:8080 \
//!   --client-id <clientId that provision-client printed> \
//!   --private-key-path /path/to/console-client-key.pem \
//!   --to +237677123456 \
//!   --sender-id VYMALO \
//!   --body "Hello from the vsms Rust example"
//! ```
//!
//! Every flag also reads from an env var (`VSMS_ISSUER`,
//! `VSMS_CLIENT_ID`, `VSMS_PRIVATE_KEY_PATH`, `VSMS_SCOPE`) so a real
//! integration never has to hardcode a credential path in argv.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

/// RFC 7523 client assertions are meant to be short-lived — long enough to
/// reach `/token`, never long enough to be useful if intercepted in
/// transit. Matches `packages/gateway/src/token.ts`'s own
/// `ASSERTION_TTL_SECONDS`.
const ASSERTION_TTL_SECONDS: i64 = 60;

/// Mint a fresh access token this many seconds before the cached one
/// actually expires, so a request never starts with a token that dies
/// mid-flight. Matches `packages/gateway/src/token.ts`'s own
/// `EXPIRY_SAFETY_MARGIN_SECONDS` exactly.
const EXPIRY_SAFETY_MARGIN_SECONDS: i64 = 60;

/// Used when the token response omits `expires_in` (optional in the OAuth2
/// response shape). Matches `token.ts`'s own fallback.
const DEFAULT_TOKEN_TTL_SECONDS: i64 = 15 * 60;

const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

#[derive(Parser, Debug)]
#[command(
    about = "vsms integration example: private_key_jwt token exchange + sendMessage over real HTTP"
)]
struct Cli {
    /// The gateway's externally reachable origin — both the OIDC issuer
    /// and `/token` hang off this. `just demo` serves it at
    /// http://127.0.0.1:8080 by default.
    #[arg(long, env = "VSMS_ISSUER", default_value = "http://127.0.0.1:8080")]
    issuer: String,

    /// The client id `sms-gateway provision-client` printed as
    /// `provisioned client: <id>`.
    #[arg(long, env = "VSMS_CLIENT_ID")]
    client_id: String,

    /// Path to the PEM private key `sms-gateway provision-client` wrote
    /// via `--key-out`. Never commit this file, and never pass its
    /// *contents* on the command line — only the path.
    #[arg(long, env = "VSMS_PRIVATE_KEY_PATH")]
    private_key_path: PathBuf,

    /// Space-separated scopes to request. Must be a subset of what the
    /// client was provisioned with (`sms-gateway provision-client
    /// --scope ...`), or `/token` mints a narrower-than-requested token.
    #[arg(long, env = "VSMS_SCOPE", default_value = "sms:send sms:read")]
    scope: String,

    /// E.164 recipient.
    #[arg(long)]
    to: String,

    /// A sender id already `approved` for this deployment's Orange
    /// account (see the main repo's `send_test_message` example, or
    /// `just demo`'s own seeding, for what "approved" requires here).
    #[arg(long)]
    sender_id: String,

    #[arg(long, default_value = "Hello from the vsms Rust integration example")]
    body: String,

    /// A caller-chosen dedupe key, forwarded as `sendMessage`'s
    /// `clientRef`. Optional — but pass the *same* value across two runs
    /// and the second is rejected as `409 Conflict` instead of sending a
    /// second real SMS. This, not an `Idempotency-Key` HTTP header, is
    /// the dedupe mechanism actually wired up in this deployment today;
    /// see this crate's own README for why.
    #[arg(long)]
    client_ref: Option<String>,
}

#[derive(Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    jti: String,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the Unix epoch")
        .as_secs() as i64
}

/// A fresh RFC 7523 §3 client assertion, signed with the caller's own
/// private key.
///
/// `kid` is set to the client id, matching `packages/gateway/src/token.ts`.
/// `authkestra_op`'s own `select_key` treats a single-key JWKS (which is
/// all `provisionAppClient` ever produces — see the main repo's AGENTS.md)
/// as unambiguous even without a `kid`, but setting it costs nothing and
/// matches what a real client should do.
///
/// `aud` is the token endpoint URL rather than the bare issuer — authkestra
/// 0.3.2+ accepts either, but matching the reference implementation
/// removes one axis of divergence to debug if the exchange ever fails.
///
/// `jti` is a fresh UUID on every call, never reused: `ClientAssertion` is
/// an insert-only table that replay-protects on this value at the
/// database (a `23505` unique-constraint violation on `record_jti`), so
/// resending the same assertion on a retry would collide with the
/// original attempt rather than repeating it.
fn sign_assertion(client_id: &str, token_endpoint: &str, key_pem: &[u8]) -> Result<String> {
    let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(key_pem)
        .context("private key is not a valid RSA PEM (PKCS#1 or PKCS#8)")?;
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(client_id.to_owned());

    let now = now_unix();
    let claims = AssertionClaims {
        iss: client_id,
        sub: client_id,
        aud: token_endpoint,
        jti: uuid::Uuid::new_v4().to_string(),
        iat: now,
        exp: now + ASSERTION_TTL_SECONDS,
    };
    jsonwebtoken::encode(&header, &claims, &encoding_key).context("signing the client assertion")
}

/// Mints and caches an access token, re-minting only once the cached one
/// is within `EXPIRY_SAFETY_MARGIN_SECONDS` of expiry. A single run of
/// this example only ever makes two authenticated calls (the send, then
/// the read-back), so caching barely matters here in isolation — but this
/// is the shape a real integration wants for the hundredth call, not just
/// the second, and it is a direct mirror of
/// `packages/gateway/src/token.ts`'s own `getAccessToken`.
struct TokenCache {
    http: reqwest::Client,
    token_endpoint: String,
    client_id: String,
    key_pem: Vec<u8>,
    scope: String,
    cached: Option<(String, i64)>,
}

impl TokenCache {
    fn new(
        http: reqwest::Client,
        issuer: &str,
        client_id: String,
        key_pem: Vec<u8>,
        scope: String,
    ) -> Self {
        Self {
            http,
            token_endpoint: format!("{}/token", issuer.trim_end_matches('/')),
            client_id,
            key_pem,
            scope,
            cached: None,
        }
    }

    async fn get(&mut self) -> Result<String> {
        if let Some((token, expires_at)) = &self.cached {
            if *expires_at > now_unix() {
                return Ok(token.clone());
            }
        }

        let assertion = sign_assertion(&self.client_id, &self.token_endpoint, &self.key_pem)?;

        let response = self
            .http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_assertion_type", CLIENT_ASSERTION_TYPE),
                ("client_assertion", assertion.as_str()),
                // Mandatory, not optional: omitting `scope` does not fall
                // back to the client's registered scopes, it mints a
                // token with `scope: None`, and this deployment's Layer-2
                // RBAC treats a missing scope as denial. Same footgun
                // `token.ts`'s own module doc calls out.
                ("scope", self.scope.as_str()),
            ])
            .send()
            .await
            .with_context(|| format!("POSTing to {}", self.token_endpoint))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading the token response body")?;
        if !status.is_success() {
            bail!(
                "token request to {} failed ({status}): {body}",
                self.token_endpoint
            );
        }
        let parsed: TokenResponse = serde_json::from_str(&body)
            .with_context(|| format!("parsing the token response as JSON: {body}"))?;

        let ttl = parsed.expires_in.unwrap_or(DEFAULT_TOKEN_TTL_SECONDS);
        let expires_at = now_unix() + (ttl - EXPIRY_SAFETY_MARGIN_SECONDS).max(0);
        println!(
            "minted access token (scope={:?}, expires in {ttl}s)",
            parsed.scope
        );
        self.cached = Some((parsed.access_token.clone(), expires_at));
        Ok(parsed.access_token)
    }
}

#[derive(Deserialize, Debug)]
struct SendMessageResult {
    #[serde(rename = "messageId")]
    message_id: String,
    state: String,
    encoding: String,
    segments: i64,
    operator: String,
    #[serde(rename = "estimatedCostXaf")]
    estimated_cost_xaf: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let key_pem = std::fs::read(&cli.private_key_path).with_context(|| {
        format!(
            "reading the private key at {}",
            cli.private_key_path.display()
        )
    })?;

    let http = reqwest::Client::builder()
        .build()
        .context("building the HTTP client")?;
    let mut tokens = TokenCache::new(
        http.clone(),
        &cli.issuer,
        cli.client_id.clone(),
        key_pem,
        cli.scope.clone(),
    );

    let access_token = tokens.get().await?;

    let mut args = serde_json::json!({
        "to": cli.to,
        "body": cli.body,
        "senderId": cli.sender_id,
    });
    if let Some(client_ref) = &cli.client_ref {
        args["clientRef"] = serde_json::Value::String(client_ref.clone());
    }

    let send_url = format!("{}/$procs/sendMessage", cli.issuer.trim_end_matches('/'));
    let response = http
        .post(&send_url)
        .bearer_auth(&access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&serde_json::json!({ "args": args }))
        .send()
        .await
        .with_context(|| format!("POSTing to {send_url}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("reading the sendMessage response body")?;

    if status == reqwest::StatusCode::CONFLICT {
        println!(
            "sendMessage returned 409 Conflict — if --client-ref was passed, that clientRef was \
             already used on a prior send. This is clientRef's database-level dedupe doing \
             exactly its job, not a bug to retry around: {body}"
        );
        return Ok(());
    }
    if !status.is_success() {
        bail!("sendMessage failed ({status}): {body}");
    }
    let result: SendMessageResult = serde_json::from_str(&body)
        .with_context(|| format!("parsing the sendMessage response as JSON: {body}"))?;

    println!();
    println!(
        "sent: messageId={} state={} encoding={} segments={} operator={} estimatedCostXaf={}",
        result.message_id,
        result.state,
        result.encoding,
        result.segments,
        result.operator,
        result.estimated_cost_xaf
    );

    // Prove the write actually landed — read it back through the REST
    // surface rather than trusting the mutation's own echoed response.
    let access_token = tokens.get().await?;
    let get_url = format!(
        "{}/messages/{}",
        cli.issuer.trim_end_matches('/'),
        result.message_id
    );
    let response = http
        .get(&get_url)
        .bearer_auth(&access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("GETting {get_url}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("reading the GET /messages/{id} response body")?;
    if !status.is_success() {
        bail!("GET /messages/{{id}} failed ({status}): {body}");
    }
    let message: serde_json::Value =
        serde_json::from_str(&body).context("parsing the GET /messages/{id} response as JSON")?;

    println!(
        "read back: id={} state={} providerMessageRef={}",
        message["id"], message["state"], message["providerMessageRef"]
    );

    Ok(())
}
