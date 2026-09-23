//! `Command::Serve` — see that variant's own doc comment in `main.rs` for
//! the flags. This file is the whole `serve` binding path: the pool, OP
//! state, the DLR router, login router assembly, both listeners, and
//! graceful shutdown.

use std::sync::Arc;

use anyhow::{Context, Result};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::GatewayAuth;
use sms_api::schema::{Cratestack, provider as provider_filter};
use sms_provider::SmsProvider;
use tracing::info;

use crate::{dlr, health, login, op, token_rate_limit};
use sms_api::system_context;

/// `Command::Serve`'s flags. See `Command::Serve`'s own doc comment in
/// `main.rs` — the enum variant carries the "why", this struct only
/// carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct ServeArgs {
    /// Address to listen on. Loopback by default: TLS terminates at a Caddy
    /// or nginx edge, and this process should never face the internet.
    #[arg(long, env = "SMS_LISTEN_ADDR", default_value = "127.0.0.1:8080")]
    pub(crate) listen: String,

    /// #70/#71: `GET /metrics`, Prometheus text exposition — bound to a
    /// **second, separate** listener, never merged into `--listen`'s own
    /// router. Loopback by default for the same reason `--listen`
    /// itself is: `deploy/Caddyfile`'s blanket `reverse_proxy
    /// sms-gateway:8080` never reaches this port at all, since it's a
    /// different port entirely — see `sms_api::metrics`'s own module
    /// doc for the full reasoning and `docs/runbooks/alerting.adoc` for
    /// how an operator points a real Prometheus at it.
    #[arg(
        long,
        env = "SMS_METRICS_LISTEN_ADDR",
        default_value = "127.0.0.1:9090"
    )]
    pub(crate) metrics_listen: String,

    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub(crate) database_url: String,

    /// Maximum pooled connections.
    #[arg(long, env = "SMS_DB_MAX_CONNECTIONS", default_value_t = 10)]
    pub(crate) max_connections: u32,

    /// The OP's own identity — every token this OP mints carries this
    /// as `iss`, and `GatewayAuth` validates incoming tokens against
    /// exactly this value. Never `listen` (a bind address, not an
    /// identity) — must be the externally reachable `https://` origin
    /// this OP is actually served at.
    #[arg(long, env = "SMS_OIDC_ISSUER")]
    pub(crate) issuer: String,

    /// `OAuth2` `client_credentials` client id for Orange Cameroon's
    /// SMS API. Optional as of #61's MTN wiring — this binary always
    /// serves the DLR route (#34), but "always needs a provider to parse
    /// against" now means *at least one* configured adapter, not
    /// specifically Orange; see [`build_dlr_router`]'s own
    /// at-least-one-provider check for the enforcement this loosening
    /// moved to. Previously required unconditionally, unlike
    /// `sms-worker`'s own copy of this flag (optional there, only needed
    /// when `dispatch` is selected) — that asymmetry is gone now that
    /// both binaries express the same "credentials are needed only if
    /// this adapter is the one in use" shape, just against a different
    /// gate (`dispatch` selection there, "no other adapter configured"
    /// here).
    #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
    pub(crate) orange_client_id: Option<String>,

    /// Paired with `orange_client_id`. Never logged.
    #[arg(long, env = "ORANGE_CM_CLIENT_SECRET", hide_env_values = true)]
    pub(crate) orange_client_secret: Option<String>,

    /// E.164 without the `tel:` scheme.
    #[arg(long, env = "ORANGE_CM_SENDER_NUMBER")]
    pub(crate) orange_sender_number: Option<String>,

    /// Overridable so a real Orange sandbox (not just this crate's own
    /// `wiremock`-backed tests) can be pointed at without a code change.
    #[arg(
        long,
        env = "ORANGE_CM_BASE_URL",
        default_value = "https://api.orange.com"
    )]
    pub(crate) orange_base_url: String,

    /// The `OAuth2` `client_credentials` client id for MTN's direct API
    /// (MADAPI) — see `sms-provider-mtn`'s module doc for the real,
    /// vendored contract this replaces the previous aggregator-shaped
    /// static-Bearer-key auth with. Optional, the same shape as the
    /// Orange trio above — this binary needs *a* provider configured,
    /// not specifically this one. Paired with the other `--mtn-*` flags
    /// below — see [`build_dlr_router`]'s own all-or-none check.
    #[arg(long, env = "MTN_CLIENT_ID")]
    pub(crate) mtn_client_id: Option<String>,

    /// Paired with `mtn_client_id`. Never logged.
    #[arg(long, env = "MTN_CLIENT_SECRET", hide_env_values = true)]
    pub(crate) mtn_client_secret: Option<String>,

    /// The approved short code MADAPI's own `outboundSMSMessageRequest.serviceCode`
    /// requires unconditionally on every submit (see
    /// `sms_provider_mtn::MtnConfig::service_code`'s own doc). Part of
    /// the all-or-none set: unlike `--mtn-sender-id` below, MADAPI's own
    /// Swagger marks this field mandatory, not optional.
    #[arg(long, env = "MTN_SERVICE_CODE")]
    pub(crate) mtn_service_code: Option<String>,

    /// The approved alphanumeric sender ID MTN submits under, used only
    /// as a fallback when a specific message's own resolved sender is
    /// empty (see `sms_provider_mtn::MtnProvider::sender_address`'s own
    /// doc). **Not** part of the all-or-none set below, unlike every
    /// other `--mtn-*` flag: MADAPI's own Swagger marks `senderAddress`
    /// optional ("This field is optional... if a senderAddress is used
    /// rather than the serviceCode, then the senderAddress value must be
    /// passed as well" — implying its *absence* is a fully valid
    /// request, using `serviceCode` alone), so requiring an operator to
    /// set this would enforce a constraint MADAPI itself doesn't have.
    #[arg(long, env = "MTN_SENDER_ID")]
    pub(crate) mtn_sender_id: Option<String>,

    /// MADAPI's API host — now a real, documented default
    /// (`https://api.mtn.com`, matching the vendored Swagger's own
    /// `host`), unlike the previous aggregator shape which had none to
    /// bake in. Not part of the all-or-none set: it always has a usable
    /// value, configured or not, the same shape `--orange-base-url`
    /// already has.
    #[arg(long, env = "MTN_BASE_URL", default_value = "https://api.mtn.com")]
    pub(crate) mtn_base_url: String,

    /// The submission rate this specific MADAPI contract allows, in
    /// messages per second — not published anywhere in the vendored
    /// Swagger (see `sms_provider_mtn::MtnConfig::tps_ceiling`'s own
    /// doc), so this stays required alongside the other `--mtn-*` flags
    /// rather than defaulted — this repo's own standing preference is no
    /// default that invents a fact.
    #[arg(long, env = "MTN_TPS_CEILING")]
    pub(crate) mtn_tps_ceiling: Option<f64>,

    /// What one segment costs on this contract, in XAF. `Decimal`, never
    /// a float — this is money. Same required-together reasoning as
    /// `--mtn-tps-ceiling` above.
    #[arg(long, env = "MTN_COST_PER_SEGMENT_XAF")]
    pub(crate) mtn_cost_per_segment_xaf: Option<rust_decimal::Decimal>,

    /// Whether this specific MADAPI relationship has an alphanumeric
    /// sender registered and approved with MTN. Defaults to `false` —
    /// the safer default per
    /// `sms_provider_mtn::MtnConfig::supports_alphanumeric_sender`'s own
    /// doc: an unregistered alphanumeric sender risks silent rewriting
    /// or dropping by MTN, not a clean rejection this adapter could
    /// classify. Not part of the all-or-none check — it has a safe
    /// default whether or not MTN is configured at all.
    #[arg(
        long,
        env = "MTN_SUPPORTS_ALPHANUMERIC_SENDER",
        default_value_t = false
    )]
    pub(crate) mtn_supports_alphanumeric_sender: bool,

    /// #134: the server-held pepper behind `Message.msisdnHash`/
    /// `Message.bodyHash` — real secret material, config only, never
    /// the database, a migration, or a log line (see `sms_api::pepper`'s
    /// module doc for the scheme and the rotation consequence).
    /// Required unconditionally and validated (minimum length) before
    /// this process does anything else, so a missing or trivially weak
    /// pepper fails loudly at startup — never silently at the first
    /// `sendMessage` call. Never logged: `HashPepper`'s own `Debug`
    /// impl redacts it even if this struct were ever printed.
    #[arg(long, env = "SMS_HASH_PEPPER", hide_env_values = true)]
    pub(crate) hash_pepper: String,

    /// #153: how long a cached `Idempotency-Key` response stays
    /// replayable before a repeat with the same key is treated as a
    /// brand-new request. Matches `docs/architecture.md` §4.5's own
    /// figure (24h) as the default.
    #[arg(long, env = "SMS_IDEMPOTENCY_TTL_SECS", default_value_t = 24 * 60 * 60)]
    pub(crate) idempotency_ttl_secs: u64,

    /// #153: per-principal token-bucket capacity for
    /// `sms_api::router`'s `RateLimitLayer` — the burst a caller can
    /// spend before throttling kicks in. Matches §4.5's own suggested
    /// default; see `sms_api::default_rate_limit_config`'s doc for why
    /// that default is safe against this workspace's actual live-suite
    /// call volume. Distinct from `/token`'s own rate limiting, which
    /// §4.2 scopes to the reverse-proxy edge instead.
    #[arg(long, env = "SMS_RATE_LIMIT_BURST", default_value_t = 120)]
    pub(crate) rate_limit_burst: u32,

    /// #153: refill rate, in tokens/second, for the same bucket.
    #[arg(long, env = "SMS_RATE_LIMIT_REFILL_PER_SECOND", default_value_t = 2.0)]
    pub(crate) rate_limit_refill_per_second: f64,

    /// #163: burst capacity for `sms_api::router`'s second, coarser
    /// `RateLimitLayer` — keyed on the real TCP peer
    /// (`ConnectInfo<SocketAddr>`, populated because this arm serves
    /// through `into_make_service_with_connect_info` below), not the
    /// unverified `sub` claim `--rate-limit-burst` above buckets by.
    /// Closes the gap that layer's own doc names: a caller willing to
    /// forge a fresh `sub` per request gets a fresh bucket from that
    /// layer alone; this one bounds the aggregate regardless. See
    /// `sms_api::default_source_rate_limit_config`'s own doc for why
    /// its default is sized differently from `--rate-limit-burst`'s.
    #[arg(long, env = "SMS_SOURCE_RATE_LIMIT_BURST", default_value_t = 1200)]
    pub(crate) source_rate_limit_burst: u32,

    /// #163: refill rate, in tokens/second, for the same bucket.
    #[arg(
        long,
        env = "SMS_SOURCE_RATE_LIMIT_REFILL_PER_SECOND",
        default_value_t = 10.0
    )]
    pub(crate) source_rate_limit_refill_per_second: f64,

    /// #168: burst capacity for the `/token` route's own defence-in-
    /// depth limiter, keyed on the real `client_id` parsed from the
    /// form-urlencoded request body — the composite dimension
    /// `docs/architecture.md` §4.2 requires and `deploy/Caddyfile`'s
    /// edge-level `token_per_ip`/`token_global` zones (#156) cannot
    /// reach (see `token_rate_limit`'s module doc for why: `/token`
    /// arrives only in the POST body, and every edge-level way to read
    /// one field out of it was checked and rejected — #168). Default
    /// mirrors `deploy/Caddyfile`'s own `token_per_ip` reasoning and
    /// figure almost exactly (20 events/minute, off the same
    /// 15-minute-token-TTL caching behaviour), expressed as a token
    /// bucket rather than a fixed window.
    #[arg(
        long,
        env = "SMS_TOKEN_RATE_LIMIT_BURST",
        default_value_t = token_rate_limit::default_token_rate_limit_config().burst
    )]
    pub(crate) token_rate_limit_burst: u32,

    /// #168: refill rate, in tokens/second, for the same bucket — 20
    /// events/minute.
    #[arg(
        long,
        env = "SMS_TOKEN_RATE_LIMIT_REFILL_PER_SECOND",
        default_value_t = token_rate_limit::default_token_rate_limit_config().refill_per_second
    )]
    pub(crate) token_rate_limit_refill_per_second: f64,

    /// #194: the `OauthClient.clientId` the human `authorization_code`
    /// login flow registers under — `GatewayAuth`'s only fixed
    /// audience to validate a human token's `aud` against (see
    /// `sms_api::auth::GatewayAuth`'s own doc for why that check can't
    /// live in the shared `Validation` both realms decode through).
    /// Must match whatever `seed-console-client` (below) provisioned.
    #[arg(
        long,
        env = "SMS_CONSOLE_OIDC_CLIENT_ID",
        default_value = sms_api::DEFAULT_CONSOLE_CLIENT_ID
    )]
    pub(crate) console_client_id: String,
}

/// `Provider.id` for the row matching `provider.key()` — resolved once at
/// startup, not re-checked per DLR callback. Safe to cache: a `Provider`
/// row's own id is immutable once created, and if the row genuinely needs
/// to change (a different key), that's a restart-worthy reconfiguration,
/// not something this route needs to notice live the way key rotation did
/// (see `op.rs`'s own module doc for the contrast — that one needed a live
/// refresh because the *key material* itself changes, not just which row
/// backs a lookup).
///
/// # Errors
///
/// No `Provider` row has this `key` yet — an operator hasn't seeded one,
/// which #34 has no CLI action for (unlike `rotate-signing-key`) since
/// seeding a `Provider` is already `provisionAppClient`-adjacent, ordinary
/// CRUD the admin console (M4) will do, not an ops action this binary
/// should grow its own subcommand for.
async fn resolve_provider_row_id(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    provider: &dyn SmsProvider,
) -> Result<String> {
    let found = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(provider.key().to_owned()),
        ))
        .limit(1)
        .run(sys)
        .await
        .context("looking up the Provider row for the configured adapter")?;

    found.into_iter().next().map(|row| row.id).with_context(|| {
        format!(
            "no Provider row has key {:?} — seed one before serving DLR callbacks",
            provider.key()
        )
    })
}

/// `Command::Serve`'s body, pulled out of `main`'s own `match` for the same
/// `clippy::too_many_lines` reason `rotate_signing_key_command`
/// (`commands::rotate_signing_key`) already was — #168 pushed `main` back
/// over the limit by adding two more CLI flags and the `token_rate_limit`
/// wiring they feed, the same shape of growth #139 caused originally.
/// Takes `ServeArgs` directly rather than the whole `Command`: `main`'s own
/// dispatch already extracts it from `Command::Serve` at the match site,
/// the same shape `provision_client_command`'s (`commands::provision_client`)
/// own doc describes.
/// The four `--orange-*` values `serve` needs to construct the adapter.
/// Grouped into one struct purely so [`build_dlr_router`] takes a single
/// argument rather than four positional `String`s of the same type, which
/// is the shape most likely to be silently mis-ordered at a call site.
struct OrangeCredentials {
    client_id: String,
    client_secret: String,
    sender_number: String,
    base_url: String,
}

/// `Ok(None)` when none of the three Orange flags are set at all — a
/// deployment that only wired up MTN. `Err` when only *some* are set —
/// mirrors `sms-worker`'s own `orange_provider`
/// (`backends/apps/sms-worker/src/main.rs`) all-or-none check exactly, since
/// both binaries build the identical `OrangeCmConfig` from the identical
/// flags, just at a different point in each one's own startup sequence.
fn orange_credentials(
    client_id: Option<String>,
    client_secret: Option<String>,
    sender_number: Option<String>,
    base_url: String,
) -> Result<Option<OrangeCredentials>> {
    match (client_id, client_secret, sender_number) {
        (Some(client_id), Some(client_secret), Some(sender_number)) => {
            Ok(Some(OrangeCredentials {
                client_id,
                client_secret,
                sender_number,
                base_url,
            }))
        }
        (None, None, None) => Ok(None),
        _ => anyhow::bail!(
            "--orange-client-id, --orange-client-secret and --orange-sender-number must all be \
             set together, or none of them"
        ),
    }
}

/// The `--mtn-*` values `serve` needs to construct the MADAPI adapter.
/// `sender_id` and `base_url` are excluded from the all-or-none tuple
/// [`mtn_credentials`] checks — see that function's own doc for why each
/// one individually has a safe, always-usable value regardless of
/// whether MTN is otherwise configured.
struct MtnCredentials {
    client_id: String,
    client_secret: String,
    service_code: String,
    sender_id: Option<String>,
    base_url: String,
    tps_ceiling: f64,
    cost_per_segment_xaf: rust_decimal::Decimal,
    supports_alphanumeric_sender: bool,
}

/// [`mtn_credentials`]'s own raw input, one field per `--mtn-*` flag —
/// grouped into a struct rather than eight positional parameters
/// because a plain eight-argument function trips clippy's own
/// `too_many_arguments` threshold (7), and a named-field struct reads
/// better at both ends than `#[allow(...)]`ing it away would. Built once
/// in [`serve_command`] straight out of `ServeArgs`'s own destructured
/// `mtn_*` fields.
struct RawMtnArgs {
    client_id: Option<String>,
    client_secret: Option<String>,
    service_code: Option<String>,
    tps_ceiling: Option<f64>,
    cost_per_segment_xaf: Option<rust_decimal::Decimal>,
    sender_id: Option<String>,
    supports_alphanumeric_sender: bool,
    base_url: String,
}

/// `Ok(None)` when none of the five required `--mtn-*` flags
/// (`client_id`/`client_secret`/`service_code`/`tps_ceiling`/
/// `cost_per_segment_xaf`) are set at all — a deployment that only wired
/// up Orange, or neither. `Err` when only *some* are set. Mirrors
/// `sms-worker::mtn_provider`'s identical check.
///
/// `sender_id` is threaded through unconditionally, not gated by this
/// match: MADAPI's own Swagger marks `senderAddress` optional (see
/// `ServeArgs::mtn_sender_id`'s own doc), so this crate must never
/// require an operator to set it. `supports_alphanumeric_sender` is
/// threaded straight from the CLI's own `default_value_t = false` for
/// the identical reason it always was — it has a safe default regardless
/// of whether MTN is configured at all. `base_url` is threaded through
/// unconditionally too, now that it carries a real default
/// (`https://api.mtn.com`) rather than needing an operator-supplied
/// value the way the previous, host-less aggregator shape did.
fn mtn_credentials(args: RawMtnArgs) -> Result<Option<MtnCredentials>> {
    let RawMtnArgs {
        client_id,
        client_secret,
        service_code,
        tps_ceiling,
        cost_per_segment_xaf,
        sender_id,
        supports_alphanumeric_sender,
        base_url,
    } = args;
    match (
        client_id,
        client_secret,
        service_code,
        tps_ceiling,
        cost_per_segment_xaf,
    ) {
        (
            Some(client_id),
            Some(client_secret),
            Some(service_code),
            Some(tps_ceiling),
            Some(cost),
        ) => Ok(Some(MtnCredentials {
            client_id,
            client_secret,
            service_code,
            sender_id,
            base_url,
            tps_ceiling,
            cost_per_segment_xaf: cost,
            supports_alphanumeric_sender,
        })),
        (None, None, None, None, None) => Ok(None),
        _ => anyhow::bail!(
            "--mtn-client-id, --mtn-client-secret, --mtn-service-code, --mtn-tps-ceiling and \
             --mtn-cost-per-segment-xaf must all be set together, or none of them"
        ),
    }
}

/// Builds however many adapters were configured and the DLR router that
/// dispatches onto whichever ones are present — at least one, since this
/// binary always serves the DLR route (#34) and a route with nothing
/// behind it can never usefully answer a callback. Both credential sets
/// are optional individually (see [`orange_credentials`]/
/// [`mtn_credentials`]); this function is what turns "neither is set" into
/// a startup failure rather than a route that 404s every real callback
/// forever.
///
/// Extracted from [`serve_command`] rather than inlined: that function
/// crossed clippy's `too_many_lines` threshold (106/100) once #194's
/// console-client wiring landed on top of the existing setup, and this is
/// the one self-contained block in it — every value it touches is
/// provider-shaped, and nothing after it reads either config or the bare
/// `provider` handles again. Suppressing the lint instead would have
/// hidden the next fifty lines of growth too.
async fn build_dlr_router(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    orange: Option<OrangeCredentials>,
    mtn: Option<MtnCredentials>,
) -> Result<axum::Router> {
    let mut providers = Vec::new();

    if let Some(orange) = orange {
        let mut orange_config = sms_provider_orange_cm::OrangeCmConfig::production(
            orange.client_id,
            orange.client_secret,
            orange.sender_number,
        );
        orange_config.base_url = orange.base_url;
        let provider: Arc<dyn SmsProvider> =
            Arc::new(sms_provider_orange_cm::OrangeCmProvider::new(orange_config));
        let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
        providers.push(dlr::DlrProvider {
            provider,
            provider_row_id,
        });
    }

    if let Some(mtn) = mtn {
        let mtn_config = sms_provider_mtn::MtnConfig {
            client_id: mtn.client_id,
            client_secret: mtn.client_secret,
            service_code: mtn.service_code,
            sender_id: mtn.sender_id,
            base_url: mtn.base_url,
            tps_ceiling: mtn.tps_ceiling,
            cost_per_segment_xaf: mtn.cost_per_segment_xaf,
            supports_alphanumeric_sender: mtn.supports_alphanumeric_sender,
            // Same values `sms-provider-orange-cm::OrangeCmConfig::production`
            // bakes in — no `MtnConfig` equivalent exists to default
            // these from, and nothing about either timeout is
            // provider-specific. Mirrors `sms-worker::mtn_provider`'s
            // identical choice.
            connect_timeout: std::time::Duration::from_secs(10),
            request_timeout: std::time::Duration::from_secs(30),
        };
        let provider: Arc<dyn SmsProvider> =
            Arc::new(sms_provider_mtn::MtnProvider::new(mtn_config));
        let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
        providers.push(dlr::DlrProvider {
            provider,
            provider_row_id,
        });
    }

    if providers.is_empty() {
        anyhow::bail!(
            "at least one provider must be configured to serve the DLR route: either \
             --orange-client-id, --orange-client-secret and --orange-sender-number, or \
             --mtn-client-id, --mtn-client-secret, --mtn-service-code, --mtn-tps-ceiling and \
             --mtn-cost-per-segment-xaf (or their env vars)"
        );
    }

    Ok(dlr::router(db.clone(), sys.clone(), providers))
}

/// Resolves both provider credential sets and builds the DLR router and
/// the `/readyz` router that needs the same pooled `db` handle. Pulled
/// out of [`serve_command`] purely to keep it under clippy's
/// `too_many_lines` limit, the same recurring growth shape
/// [`build_dlr_router`]'s and [`build_op_state`]'s own doc comments
/// already record — this time it was MTN's own `--mtn-client-secret`/
/// `--mtn-service-code` split landing on top of an already-tight budget.
/// Seven parameters, not eight: `mtn`'s own five raw CLI values are
/// already grouped into [`RawMtnArgs`] (see that struct's own doc for
/// why), so this function doesn't just move the `too_many_arguments`
/// problem one level up.
async fn build_provider_routers(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    orange_client_id: Option<String>,
    orange_client_secret: Option<String>,
    orange_sender_number: Option<String>,
    orange_base_url: String,
    mtn: RawMtnArgs,
) -> Result<(axum::Router, axum::Router)> {
    let orange = orange_credentials(
        orange_client_id,
        orange_client_secret,
        orange_sender_number,
        orange_base_url,
    )?;
    let mtn = mtn_credentials(mtn)?;
    let dlr_router = build_dlr_router(db, sys, orange, mtn).await?;
    // #157: /readyz needs the same pooled handle every other router
    // shares — cloned here, before `sms_api::router` (in `serve_command`)
    // takes `db` by value as its own last use.
    let health_router = health::router(db.clone());
    Ok((dlr_router, health_router))
}

/// Loads the OP's signing keys, assembles its state, and starts the
/// background key refresh.
///
/// Extracted from [`serve_command`] for the same reason
/// [`build_dlr_router`] was — that function sits against clippy's
/// `too_many_lines` ceiling, and this is a self-contained block whose
/// values nothing downstream reads individually (only `op_state`).
///
/// Note this **fails at process start**, before the listener binds, if no
/// active signing key exists — not lazily on the first `/token` request.
/// That ordering is load-bearing for deployment: anything waiting for the
/// gateway to be healthy before rotating a key would deadlock, which is
/// why the deploy runbook uses `docker compose run --rm` rather than
/// `exec`.
async fn build_op_state(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    issuer: &str,
) -> Result<op::OpState> {
    let (signing, jwks) = sms_auth::op::load_signing_keys(db, sys, issuer)
        .await
        .context(
            "loading OP signing keys — run `sms-gateway rotate-signing-key` if this is a fresh \
             database",
        )?;
    let op_store = sms_auth::op::machine_only_store(std::sync::Arc::new(db.clone()), sys.clone());
    let op_config = sms_auth::op::machine_only_config(issuer.to_owned());
    let op_state = op::OpState::new(op_store, signing, op_config, jwks);
    // Keeps a rotate-signing-key run against this already-running process
    // from silently never taking effect — see op.rs's own module doc.
    op::spawn_key_refresh(
        op_state.clone(),
        db.clone(),
        sys.clone(),
        issuer.to_owned(),
        op::DEFAULT_KEY_REFRESH_INTERVAL,
    );
    Ok(op_state)
}

pub(crate) async fn serve_command(args: ServeArgs) -> Result<()> {
    // Deliberately not one big `let ServeArgs { .. } = args;` up front —
    // that pattern alone used to span 27 source lines, and clippy's own
    // `too_many_lines` counts them. Every field the two `.await`s below
    // need (`hash_pepper`, `database_url`, `max_connections`, `issuer`,
    // every `orange_*`/`mtn_*` flag) is read or moved straight off `args`
    // instead; the one destructure at the bottom, once `args` no longer
    // needs to be a value the two helpers below can still read from,
    // binds only what's left — a genuine reduction in this function's own
    // line count, not just a relocation of the same lines. Partial moves
    // off an owned, non-`Drop` struct are ordinary, sound Rust: nothing
    // here reads `args` as a whole again after any individual field is
    // taken.
    //
    // #134: validated before anything else in this branch runs — failing
    // loudly on a missing/too-short pepper at startup, not at the first
    // `sendMessage` call. `clap`'s own `env`/required handling already
    // refuses a *missing* value before this line is ever reached; this is
    // the length check clap can't express.
    let pepper = sms_api::HashPepper::new(args.hash_pepper.clone())
        .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

    let pool = PgPoolOptions::new()
        .max_connections(args.max_connections)
        .connect(&args.database_url)
        .await
        .context("connecting to Postgres")?;

    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:op");

    // #38/#39: this process's `Message` writes (`sendMessage`, DLR
    // ingestion) are the only ones this milestone wires a webhook
    // subscriber for. Registering before anything else touches `db` is
    // required, not just tidy — a write on an emitting model with no
    // subscriber registered on this process's own `Cratestack` instance
    // doesn't wait for `drain` to catch it later; the library's own
    // automatic post-commit drain marks it delivered with nothing done,
    // silently, the moment the write commits. See `sms_api::webhooks`'s
    // own module doc for the full mechanism.
    sms_api::webhooks::register_subscribers(&db);

    let op_state = build_op_state(&db, &sys, &args.issuer).await?;

    let (dlr_router, health_router) = build_provider_routers(
        &db,
        &sys,
        args.orange_client_id.clone(),
        args.orange_client_secret.clone(),
        args.orange_sender_number.clone(),
        args.orange_base_url.clone(),
        RawMtnArgs {
            client_id: args.mtn_client_id.clone(),
            client_secret: args.mtn_client_secret.clone(),
            service_code: args.mtn_service_code.clone(),
            tps_ceiling: args.mtn_tps_ceiling,
            cost_per_segment_xaf: args.mtn_cost_per_segment_xaf,
            sender_id: args.mtn_sender_id.clone(),
            supports_alphanumeric_sender: args.mtn_supports_alphanumeric_sender,
            base_url: args.mtn_base_url.clone(),
        },
    )
    .await?;

    let ServeArgs {
        listen,
        metrics_listen,
        issuer,
        console_client_id,
        idempotency_ttl_secs,
        rate_limit_burst,
        rate_limit_refill_per_second,
        source_rate_limit_burst,
        source_rate_limit_refill_per_second,
        token_rate_limit_burst,
        token_rate_limit_refill_per_second,
        ..
    } = args;

    let auth = GatewayAuth::new(
        db.clone(),
        format!("{issuer}/jwks.json"),
        issuer,
        console_client_id.clone(),
    );
    // #168: the /token route's own client_id-keyed defence-in-depth
    // limiter — distinct from sms_api::router's two, which never wrap
    // /token at all (see that function's own doc). See
    // token_rate_limit's own module doc for why this belongs here and not
    // in deploy/Caddyfile or authkestra-op.
    let token_rate_limit =
        token_rate_limit::TokenRateLimitState::new(cratestack::ratelimit::RateLimitConfig::new(
            token_rate_limit_burst,
            token_rate_limit_refill_per_second,
        ));
    // #194: built before `sms_api::router` below takes `db`/`sys` by value —
    // same ordering constraint `dlr::router` above is already subject to.
    let login_router = login::router(db.clone(), sys, op_state.clone());

    let app = sms_api::router(
        db,
        auth,
        pepper,
        std::time::Duration::from_secs(idempotency_ttl_secs),
        cratestack::ratelimit::RateLimitConfig::new(rate_limit_burst, rate_limit_refill_per_second),
        cratestack::ratelimit::RateLimitConfig::new(
            source_rate_limit_burst,
            source_rate_limit_refill_per_second,
        ),
    )
    .merge(op::router(op_state, token_rate_limit))
    .merge(dlr_router)
    .merge(health_router)
    .merge(login_router);

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    info!(listen = %listen, "sms-gateway listening");

    // #70/#71: a genuinely second listener, never merged into `app` above
    // — see `metrics_listen`'s own doc and `sms_api::metrics`'s module doc
    // for why. Spawned before the main `serve` call below so a bind
    // failure here (a port already in use, an invalid address) is caught
    // and surfaces the same way any other startup failure does, rather
    // than silently never having bound at all.
    let metrics_server = spawn_metrics_server(&metrics_listen).await?;

    // #163: `sms_api::router`'s coarser, `ConnectInfo`-keyed
    // `RateLimitLayer` (see that module's `source_fingerprint` doc) only
    // sees a real peer address when served through this — plain
    // `into_make_service()` leaves it permanently absent, silently
    // collapsing that layer to its shared-bucket fallback for every
    // caller, not just forged ones.
    cratestack::axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serving HTTP")?;

    // The main listener above already returned (graceful shutdown
    // completed) by the time execution reaches here — wait for the metrics
    // listener's own identical shutdown to finish too, so this process
    // doesn't exit out from under a task still mid-`accept`.
    metrics_server
        .await
        .context("metrics server task panicked")?
        .context("serving metrics HTTP")?;
    Ok(())
}

/// Binds `metrics_listen` and spawns `sms_api::metrics::router()` on it,
/// tied to the same [`shutdown_signal`] every other listener in this binary
/// uses. Pulled out of [`serve_command`] purely to stay under clippy's
/// `too_many_lines` limit — see `main.rs`'s own git history for the
/// established convention of extracting an arm rather than raising the
/// limit (`16db8db`, for `rotate-signing-key`'s own arm).
async fn spawn_metrics_server(
    metrics_listen: &str,
) -> Result<tokio::task::JoinHandle<std::io::Result<()>>> {
    let metrics_listener = tokio::net::TcpListener::bind(metrics_listen)
        .await
        .with_context(|| format!("binding metrics listener {metrics_listen}"))?;
    info!(listen = %metrics_listen, "sms-gateway metrics listening");
    Ok(tokio::spawn(async move {
        cratestack::axum::serve(
            metrics_listener,
            sms_api::metrics::router().into_make_service(),
        )
        .with_graceful_shutdown(shutdown_signal())
        .await
    }))
}

/// Resolve on SIGINT *or* SIGTERM so in-flight requests finish.
///
/// `ctrl_c()` alone only catches SIGINT. §9.2 deploys this as a Docker
/// container, and `docker stop` / `kubectl rollout restart` send SIGTERM
/// first, SIGKILL only after the grace period elapses — SIGINT is never
/// sent in that path at all. Missing SIGTERM here would mean this branch
/// never fires under the deployment §9.2 actually describes, and the
/// process would always hit the force-kill timeout instead, silently,
/// since a container restarting slightly late looks identical to one
/// restarting correctly.
///
/// Unix-only because `tokio::signal::unix` is: §9.2's deployment is Docker
/// Compose on a single VM, never Windows, so a `cfg(unix)` split with a
/// SIGINT-only fallback elsewhere costs nothing this binary needs.
///
/// Milestone 2 adds the advisory-lock release here — `Drop` cannot do it,
/// because releasing needs an `await`.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).expect("installing a SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::{RawMtnArgs, mtn_credentials, orange_credentials};

    #[test]
    fn orange_credentials_is_none_when_all_three_are_unset() {
        let result = orange_credentials(None, None, None, "https://api.orange.com".to_owned())
            .expect("all-unset is not an error");
        assert!(result.is_none());
    }

    #[test]
    fn orange_credentials_is_some_when_all_three_are_set() {
        let result = orange_credentials(
            Some("id".to_owned()),
            Some("secret".to_owned()),
            Some("+237677000000".to_owned()),
            "https://api.orange.com".to_owned(),
        )
        .expect("all-set is not an error")
        .expect("all-set must produce Some");
        assert_eq!(result.client_id, "id");
        assert_eq!(result.client_secret, "secret");
        assert_eq!(result.sender_number, "+237677000000");
        assert_eq!(result.base_url, "https://api.orange.com");
    }

    #[test]
    fn orange_credentials_rejects_a_partial_set() {
        let error = orange_credentials(
            Some("id".to_owned()),
            None,
            None,
            "https://api.orange.com".to_owned(),
        )
        // `OrangeCredentials` (the `Ok` type) isn't `Debug`, so
        // `.expect_err(...)` doesn't compile here — `.err()` sidesteps
        // that: `Result::err` needs no bound on `T` at all.
        .err()
        .expect("only one of three set must be rejected, not silently treated as unset");
        assert!(error.to_string().contains("orange-client-id"), "{error}");
    }

    /// Every field of [`RawMtnArgs`] set to its "nothing configured"
    /// value — individual tests below override just the fields under
    /// test via struct-update syntax, the same `base_cli()` shape
    /// `sms-worker`'s own `main.rs` test module already uses.
    fn base_mtn_args() -> RawMtnArgs {
        RawMtnArgs {
            client_id: None,
            client_secret: None,
            service_code: None,
            tps_ceiling: None,
            cost_per_segment_xaf: None,
            sender_id: None,
            supports_alphanumeric_sender: false,
            base_url: "https://api.mtn.com".to_owned(),
        }
    }

    #[test]
    fn mtn_credentials_is_none_when_all_five_are_unset() {
        let result = mtn_credentials(base_mtn_args()).expect("all-unset is not an error");
        assert!(result.is_none());
    }

    #[test]
    fn mtn_credentials_is_some_when_all_five_are_set() {
        let result = mtn_credentials(RawMtnArgs {
            client_id: Some("client-id".to_owned()),
            client_secret: Some("client-secret".to_owned()),
            service_code: Some("131".to_owned()),
            tps_ceiling: Some(20.0),
            cost_per_segment_xaf: Some(rust_decimal::Decimal::new(15, 0)),
            sender_id: Some("SENDER".to_owned()),
            supports_alphanumeric_sender: true,
            ..base_mtn_args()
        })
        .expect("all-set is not an error")
        .expect("all-set must produce Some");
        assert_eq!(result.client_id, "client-id");
        assert_eq!(result.client_secret, "client-secret");
        assert_eq!(result.service_code, "131");
        assert_eq!(result.sender_id.as_deref(), Some("SENDER"));
        assert_eq!(result.base_url, "https://api.mtn.com");
        assert!((result.tps_ceiling - 20.0).abs() < f64::EPSILON);
        assert_eq!(
            result.cost_per_segment_xaf,
            rust_decimal::Decimal::new(15, 0)
        );
        assert!(result.supports_alphanumeric_sender);
    }

    #[test]
    fn mtn_credentials_rejects_a_partial_set() {
        let error = mtn_credentials(RawMtnArgs {
            client_id: Some("client-id".to_owned()),
            client_secret: Some("client-secret".to_owned()),
            ..base_mtn_args()
        })
        .err()
        .expect("only two of five set must be rejected, not silently treated as unset");
        assert!(error.to_string().contains("mtn-client-id"), "{error}");
    }

    /// `sender_id` must never gate the all-or-none check — MADAPI's own
    /// Swagger marks `senderAddress` optional (see [`mtn_credentials`]'s
    /// own doc). Guard-failure proof: a caller that sets it while every
    /// other `--mtn-*` flag is unset must still resolve to `None`, not
    /// `Err`.
    #[test]
    fn mtn_credentials_unset_with_only_sender_id_set_is_still_none_not_an_error() {
        let result = mtn_credentials(RawMtnArgs {
            sender_id: Some("SENDER".to_owned()),
            ..base_mtn_args()
        })
        .expect("sender_id alone must not trip the all-or-none check");
        assert!(result.is_none());
    }

    /// `supports_alphanumeric_sender` must never gate the all-or-none
    /// check either — it has a safe default regardless of whether MTN is
    /// configured at all (see [`mtn_credentials`]'s own doc). This is the
    /// guard-failure proof for that claim: a caller that sets it `true`
    /// while every other `--mtn-*` flag is unset must still resolve to
    /// `None`, not `Err`.
    #[test]
    fn mtn_credentials_unset_with_alphanumeric_true_is_still_none_not_an_error() {
        let result = mtn_credentials(RawMtnArgs {
            supports_alphanumeric_sender: true,
            ..base_mtn_args()
        })
        .expect("supports_alphanumeric_sender alone must not trip the all-or-none check");
        assert!(result.is_none());
    }
}
