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

    #[arg(long, env = "DATABASE_URL")]
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
    /// SMS API — required unconditionally, unlike `sms-worker`'s own
    /// copy of this flag (optional there, only needed when `dispatch`
    /// is selected): this binary always serves the DLR route (#34),
    /// which always needs a provider to parse against.
    #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
    pub(crate) orange_client_id: String,

    /// Paired with `orange_client_id`. Never logged.
    #[arg(long, env = "ORANGE_CM_CLIENT_SECRET")]
    pub(crate) orange_client_secret: String,

    /// E.164 without the `tel:` scheme.
    #[arg(long, env = "ORANGE_CM_SENDER_NUMBER")]
    pub(crate) orange_sender_number: String,

    /// Overridable so a real Orange sandbox (not just this crate's own
    /// `wiremock`-backed tests) can be pointed at without a code change.
    #[arg(
        long,
        env = "ORANGE_CM_BASE_URL",
        default_value = "https://api.orange.com"
    )]
    pub(crate) orange_base_url: String,

    /// #134: the server-held pepper behind `Message.msisdnHash`/
    /// `Message.bodyHash` — real secret material, config only, never
    /// the database, a migration, or a log line (see `sms_api::pepper`'s
    /// module doc for the scheme and the rotation consequence).
    /// Required unconditionally and validated (minimum length) before
    /// this process does anything else, so a missing or trivially weak
    /// pepper fails loudly at startup — never silently at the first
    /// `sendMessage` call. Never logged: `HashPepper`'s own `Debug`
    /// impl redacts it even if this struct were ever printed.
    #[arg(long, env = "SMS_HASH_PEPPER")]
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

impl OrangeCredentials {
    fn new(
        client_id: String,
        client_secret: String,
        sender_number: String,
        base_url: String,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            sender_number,
            base_url,
        }
    }
}

/// Builds the Orange adapter and the DLR router that dispatches onto it.
///
/// Extracted from [`serve_command`] rather than inlined: that function
/// crossed clippy's `too_many_lines` threshold (106/100) once #194's
/// console-client wiring landed on top of the existing setup, and this is
/// the one self-contained block in it — every value it touches is
/// provider-shaped, and nothing after it reads `orange_config` or the
/// bare `provider` handle again. Suppressing the lint instead would have
/// hidden the next fifty lines of growth too.
async fn build_dlr_router(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    orange: OrangeCredentials,
) -> Result<axum::Router> {
    let mut orange_config = sms_provider_orange_cm::OrangeCmConfig::production(
        orange.client_id,
        orange.client_secret,
        orange.sender_number,
    );
    orange_config.base_url = orange.base_url;
    let provider: Arc<dyn SmsProvider> =
        Arc::new(sms_provider_orange_cm::OrangeCmProvider::new(orange_config));
    let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
    Ok(dlr::router(
        db.clone(),
        sys.clone(),
        provider,
        provider_row_id,
    ))
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
    let ServeArgs {
        listen,
        metrics_listen,
        database_url,
        max_connections,
        issuer,
        orange_client_id,
        orange_client_secret,
        orange_sender_number,
        orange_base_url,
        hash_pepper,
        idempotency_ttl_secs,
        rate_limit_burst,
        rate_limit_refill_per_second,
        source_rate_limit_burst,
        source_rate_limit_refill_per_second,
        token_rate_limit_burst,
        token_rate_limit_refill_per_second,
        console_client_id,
    } = args;

    // #134: validated before anything else in this branch runs — failing
    // loudly on a missing/too-short pepper at startup, not at the first
    // `sendMessage` call. `clap`'s own `env`/required handling already
    // refuses a *missing* value before this line is ever reached; this is
    // the length check clap can't express.
    let pepper = sms_api::HashPepper::new(hash_pepper)
        .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(&database_url)
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

    let op_state = build_op_state(&db, &sys, &issuer).await?;

    let orange = OrangeCredentials::new(
        orange_client_id,
        orange_client_secret,
        orange_sender_number,
        orange_base_url,
    );
    let dlr_router = build_dlr_router(&db, &sys, orange).await?;
    // #157: /readyz needs the same pooled handle every other router
    // shares — cloned here, before `sms_api::router` below takes `db` by
    // value as its own last use.
    let health_router = health::router(db.clone());

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
