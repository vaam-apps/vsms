//! `Command::Serve` — see that variant's own doc comment in `main.rs` for
//! the flags. This file is the whole `serve` binding path: the pool, the
//! signed-transport keys, the DLR router, both listeners, and graceful
//! shutdown.

use std::sync::Arc;

use anyhow::{Context, Result};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, provider as provider_filter};
use sms_api::{BearerVerifier, HmacWebhooks};
use sms_auth::op::ServedGrants;
use sms_config::{AuthMethod, Binary, Providers};
use sms_provider::SmsProvider;
use tracing::{info, warn};

use crate::commands::server_keys;
use crate::op_bearer::{DbKeys, OpBearerVerifier, Realms};
use crate::{dlr, health, login, op, sso, token_rate_limit};
use sms_api::system_context;

/// `Command::Serve`'s flags. See `Command::Serve`'s own doc comment in
/// `main.rs` — the enum variant carries the "why", this struct only
/// carries the flags themselves.
///
/// Process plumbing only (VSC-1): where to listen, where the database and the
/// signing keys are, and which YAML configuration to read. Everything else a
/// deployment decides - the pepper, the pool size, the rate limits, every
/// provider - is in `config/application*.yml` (`sms_config`).
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

    /// The signed transport's server keys (VS-3): every generated route
    /// answers only a `COSE_Sign1`-sealed request, and seals its answer with
    /// `--envelope-signing-key-file`. Both files are required; the process
    /// does not start without them.
    #[command(flatten)]
    pub(crate) envelope_keys: crate::commands::server_keys::EnvelopeKeyArgs,

    /// The YAML deployment configuration (VSC-1).
    #[command(flatten)]
    pub(crate) config: sms_config::ConfigArgs,
}

/// `Provider.id` for the row matching `provider.key()` — resolved once at
/// startup, not re-checked per DLR callback. Safe to cache: a `Provider`
/// row's own id is immutable once created, and if the row genuinely needs
/// to change (a different key), that's a restart-worthy reconfiguration.
///
/// # Errors
///
/// No `Provider` row has this `key` yet — an operator hasn't seeded one
/// (`sms-gateway seed-dispatch`, or `bootstrap`).
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

/// Builds however many adapters were configured and the DLR router that
/// dispatches onto whichever ones are present — at least one, since this
/// binary always serves the DLR route (#34) and a route with nothing
/// behind it can never usefully answer a callback. Each provider is
/// optional individually (an absent `providers` entry); this function is what
/// turns "none is configured" into a startup failure rather than a route that
/// 404s every real callback forever.
///
/// Every adapter's config comes from `sms_config`, the same conversion the
/// worker uses, so the two binaries cannot build a provider differently.
async fn build_dlr_router(
    db: &Cratestack,
    sys: &cratestack::CratestackContext,
    providers_config: &Providers,
    profile: &str,
) -> Result<axum::Router> {
    let mut providers = Vec::new();

    if let Some(aws_sms) = aws_sms_receiver(providers_config.aws_sms.as_ref(), profile)? {
        let provider: Arc<dyn SmsProvider> = Arc::new(aws_sms);
        let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
        providers.push(dlr::DlrProvider {
            provider,
            provider_row_id,
        });
    }

    if let Some(orange) = &providers_config.orange_cm {
        let provider: Arc<dyn SmsProvider> = Arc::new(
            sms_provider_orange_cm::OrangeCmProvider::new(orange.to_provider_config()),
        );
        let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
        providers.push(dlr::DlrProvider {
            provider,
            provider_row_id,
        });
    }

    if let Some(mtn) = &providers_config.mtn_cm {
        let provider: Arc<dyn SmsProvider> =
            Arc::new(sms_provider_mtn::MtnProvider::new(mtn.to_provider_config()));
        let provider_row_id = resolve_provider_row_id(db, sys, provider.as_ref()).await?;
        providers.push(dlr::DlrProvider {
            provider,
            provider_row_id,
        });
    }

    if providers.is_empty() {
        anyhow::bail!(
            "at least one provider must be configured to serve the DLR route: a `providers` entry with `code: orange_cm` or `code: mtn_cm`, or `code: aws_sms` with `settings.events_topic_arn` (config/application*.yml)"
        );
    }

    Ok(dlr::router(db.clone(), sys.clone(), providers))
}

/// The AWS receipt receiver, when an events topic is configured.
///
/// In a `fake-aws` build, `settings.fake_sns_cert_base` swaps where signing
/// certificates are fetched from; see [`fake_aws_receiver`]. A release build
/// has neither the key's effect nor the code: it refuses the key, as it
/// refused the flag (the flag did not parse), so no deployment of it can be
/// told to trust a certificate authority that is not AWS's.
fn aws_sms_receiver(
    aws: Option<&sms_config::AwsSms>,
    profile: &str,
) -> Result<Option<sms_provider_aws_sms::AwsSmsProvider>> {
    let Some(aws) = aws else {
        return Ok(None);
    };
    #[cfg(feature = "fake-aws")]
    {
        // Only the release build's refusal below names the profile.
        let _ = profile;
        tracing::warn!(
            "this sms-gateway was built with the `fake-aws` feature, for demos and tests only; a release image is never built with it"
        );
        if let Some(base) = aws.fake_sns_cert_base.as_deref() {
            return fake_aws_receiver(aws.events_topic_arn.clone(), base).map(Some);
        }
    }
    #[cfg(not(feature = "fake-aws"))]
    if aws.fake_sns_cert_base.is_some() {
        // The profile is named because the usual cause is the wrong one: a
        // release image run without SMS_PROFILE loads the `dev` profile, whose
        // overlay points at the local fakes.
        anyhow::bail!(
            "providers[code=aws_sms].settings.fake_sns_cert_base is set, but this sms-gateway was built without the `fake-aws` feature and cannot be told to trust a fake SNS certificate authority. Remove the key. The `{profile}` profile is loaded; if that is not the profile you meant, set SMS_PROFILE (or pass --profile)."
        );
    }
    aws.events_topic_arn
        .clone()
        .map(|topic_arn| {
            sms_provider_aws_sms::AwsSmsProvider::receiver(topic_arn)
                .context("providers[code=aws_sms].settings.events_topic_arn")
        })
        .transpose()
}

/// A receiver that trusts SNS signing certificates served by
/// `sms-fake-aws` at `cert_base`, and so accepts receipts it signed. Only
/// the certificate's source changes: the topic, the signature version, the
/// certificate URL's shape and the signature itself are all still checked.
#[cfg(feature = "fake-aws")]
pub(crate) fn fake_aws_receiver(
    topic_arn: Option<String>,
    cert_base: &str,
) -> Result<sms_provider_aws_sms::AwsSmsProvider> {
    let topic_arn = topic_arn.context(
        "providers[code=aws_sms].settings.fake_sns_cert_base needs \
         providers[code=aws_sms].settings.events_topic_arn: it changes how AWS receipts are \
         verified, and without a topic there are none",
    )?;
    let http = sms_fake_aws::RedirectSnsHttp::new(cert_base)
        .map_err(anyhow::Error::msg)
        .context("providers[code=aws_sms].settings.fake_sns_cert_base")?;
    tracing::warn!(
        cert_base,
        topic_arn,
        "TRUSTING SNS SIGNING CERTIFICATES FROM sms-fake-aws: receipts on /dlr/aws_sms are \
         accepted if the fake signed them. Never run this against a real deployment."
    );
    sms_provider_aws_sms::AwsSmsProvider::receiver_with(topic_arn, Arc::new(http))
        .context("providers[code=aws_sms].settings.events_topic_arn")
}

pub(crate) async fn serve_command(args: ServeArgs) -> Result<()> {
    // VSC-1: the configuration is read before anything else is touched - before
    // the keys, before the database. A retired variable, an unset `${VAR}` or a
    // provider with a key missing stops the process here, naming the YAML key,
    // and nothing has been connected to yet.
    let config = args
        .config
        .load(Binary::Gateway)
        .context("loading the YAML configuration")?;
    info!(
        deployment = %config.deployment.name,
        livemode = config.deployment.livemode,
        public_base_url = %config.deployment.public_base_url,
        profile = %args.config.profile,
        "configuration loaded"
    );

    // VSA-1: which ways of authenticating this deployment permits. Absent is
    // all four; the line is where an operator who believes they narrowed the list
    // sees that they did not.
    let hmac_webhooks =
        HmacWebhooks::from_permitted(config.auth_methods.permits(AuthMethod::HmacWebhooks));
    log_auth_methods(&config.auth_methods);

    // #134: validated before anything else in this branch runs — failing
    // loudly on a missing/too-short pepper at startup, not at the first
    // `sendMessage` call. The loader refuses a missing or unset value; this is
    // the length check it can't express.
    let pepper = sms_api::HashPepper::new(config.security()?.hash_pepper.expose().to_owned())
        .context("security.hash_pepper is invalid — see sms_api::pepper's module doc")?;

    // VS-3: the same fail-fast stance for the signed transport's keys. A
    // gateway that cannot seal its answers must not start, and there is no
    // unsigned mode to fall back to; `clap` already refuses a missing flag,
    // this refuses an unreadable, malformed or duplicated key.
    let server_keys = args.envelope_keys.load()?;
    server_keys::log_public_keys(&server_keys);

    let pool = PgPoolOptions::new()
        .max_connections(config.database.gateway_max_connections)
        .connect(&args.database_url)
        .await
        .context("connecting to Postgres")?;

    let db = Cratestack::builder(pool).build();
    let sys = system_context("sms-gateway:serve");

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

    // VSA-1: how many `hmac` endpoints exist and what this process does about
    // them. After the registration above and before anything is served; a failed
    // count is logged, never fatal.
    match sms_api::HmacBoot::read(&db, &sys, hmac_webhooks).await {
        Ok(boot) if boot.is_warning() => {
            warn!(
                hmac_endpoints = boot.count,
                hmac_webhooks = false,
                "{}",
                boot.message()
            );
        }
        Ok(boot) => {
            info!(
                hmac_endpoints = boot.count,
                hmac_webhooks = boot.hmac.is_permitted(),
                "{}",
                boot.message()
            );
        }
        Err(error) => warn!(%error, "counting hmac webhook endpoints at boot failed"),
    }

    let dlr_router = build_dlr_router(&db, &sys, &config.providers, &args.config.profile).await?;
    // #157: /readyz needs the same pooled handle every other router
    // shares — cloned here, before `sms_api::router` takes `db` by value as
    // its own last use.
    let health_router = health::router(db.clone());
    // VSA-3a/4a: the `OpenID` Provider, mounted only when `machine_tokens` or
    // `human_login` is served (permitted and its block written). Built before
    // `sms_api::router` takes `db`. VSA-3b: the same provider's keys are what the
    // API verifies a bearer with.
    let (op_router, doors) = build_op_and_doors(&db, &config).await?;

    let ServeArgs {
        listen,
        metrics_listen,
        ..
    } = args;
    let api = config.api;
    let auth_discovery = crate::auth_discovery::router(&config.auth_methods);

    let app = sms_api::router(
        db,
        server_keys.into_signer(),
        pepper,
        std::time::Duration::from_secs(api.idempotency_ttl_secs),
        cratestack::ratelimit::RateLimitConfig::new(
            api.rate_limit.burst,
            api.rate_limit.refill_per_second,
        ),
        cratestack::ratelimit::RateLimitConfig::new(
            api.source_rate_limit.burst,
            api.source_rate_limit.refill_per_second,
        ),
        sms_api::ApiAuth {
            hmac_webhooks,
            doors,
        },
    )
    .context("assembling the signed-transport router")?
    // Everything merged from here on sits OUTSIDE the envelope layer, which
    // `sms_api::router` applied to its own routes only: the provider
    // callbacks (a provider cannot sign) and the probes. See
    // `sms_api::router::router`'s doc. The provider's own routes are merged
    // below, when served; the one document always here is the list of
    // authentication methods (`/.well-known/vsms-auth`, VSA-1), which a client
    // must be able to read before it can sign.
    .merge(dlr_router)
    .merge(health_router)
    .merge(auth_discovery);
    // `/token`, `/jwks.json`, the discovery document and `POST /login` sit outside
    // the envelope and the bearer door too: a caller of `/token` has no key to
    // sign with and no token yet, it is asking for one, and a person signing in
    // is proving who they are. With neither method served they are not routes at
    // all (404).
    let app = match op_router {
        Some(op_router) => app.merge(op_router),
        None => app,
    };

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
/// limit.
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

/// The provider's routes, when `machine_tokens` or `human_login` is served, and
/// the doors the API opens: the signed envelope when `keypair` is permitted, a
/// bearer token exactly when `machine_tokens` or `human_login` is served.
async fn build_op_and_doors(
    db: &Cratestack,
    config: &sms_config::Config,
) -> Result<(Option<axum::Router>, sms_api::ApiDoors)> {
    let (op_router, bearer) = match build_op_router(db, config).await? {
        Some((router, verifier)) => (Some(router), verifier),
        None => (None, None),
    };
    let doors = doors(
        config.auth_methods.permits(AuthMethod::Keypair),
        ServedGrants::from_methods(&config.auth_methods),
        bearer,
    )
    .context("auth.methods opens no way into the API")?;
    Ok((op_router, doors))
}

/// The doors the API opens, with the one rule `ApiDoors::new` cannot know: a
/// person's token is **not a way in on its own**. `ApiDoors::new` sees only a
/// verifier, and a verifier holding the person realm alone would build a gateway
/// reachable by people and nobody else (`Off`, no signed door). The
/// configuration refuses that first (`ConfigError::NoWayIn`,
/// `can_authenticate_a_caller`); this backs it up where the doors are built, so
/// a future caller of `serve` that builds the sets some other way still cannot.
///
/// # Errors
///
/// [`sms_api::NoDoor`] when neither the keypair nor `machine_tokens` is open.
fn doors(
    keypair: bool,
    served: ServedGrants,
    bearer: Option<Arc<dyn BearerVerifier>>,
) -> Result<sms_api::ApiDoors, sms_api::NoDoor> {
    if !keypair && !served.machine_tokens {
        return Err(sms_api::NoDoor);
    }
    sms_api::ApiDoors::new(keypair, bearer)
}

/// Loads the OP's signing keys, assembles its state, checks the console client
/// and starts the background key refresh, **when either method that rides the
/// provider is served**; `Ok(None)` mounts nothing.
///
/// The verifier is `Some` when either realm is served ([`realms`]): a machine's
/// token for `machine_tokens`, a person's access token (addressed to the
/// console, and never an `owner`'s or an `admin`'s: VSA-4b) for `human_login`.
/// `human_login` alone is not a way in (`ConfigError::NoWayIn`), so it only ever
/// reaches here beside the keypair or `machine_tokens`.
///
/// Fails at process start if no active signing key exists, or the console client
/// row is missing, differs from `human_login.console_client` or carries the
/// refresh grant, not lazily on the first request: a gateway that cannot sign a
/// token, or whose sign-in would be refused by its own provider, must not claim to
/// issue them. Anything that waits for health before rotating a key would
/// deadlock, so a fresh deployment runs `rotate-signing-key` and
/// `seed-console-client` first (`run --rm`, never `exec`).
async fn build_op_router(
    db: &Cratestack,
    config: &sms_config::Config,
) -> Result<Option<(axum::Router, Option<Arc<dyn BearerVerifier>>)>> {
    if !config.auth_methods.op_served() {
        return Ok(None);
    }
    // The provider's own actor label, as in `29f93c4^`: it lands verbatim in
    // `cratestack_audit.actor.sub` on every signing-key and assertion write, so
    // those rows name the OP and not the whole `serve` process.
    let sys = system_context("sms-gateway:op");
    let op_config = config
        .op
        .as_ref()
        .context("op: is missing although the provider is served")?;
    let served = ServedGrants::from_methods(&config.auth_methods);
    let (signing, jwks) = sms_auth::op::load_signing_keys(db, &sys, &op_config.issuer)
        .await
        .context(
            "loading the OP signing keys — run `sms-gateway rotate-signing-key` if this is a \
             fresh database",
        )?;
    if let Some(human) = config
        .human_login
        .as_ref()
        .filter(|_| config.auth_methods.human_login_served())
    {
        sms_auth::console_client::check_console_client(db, &sys, &human.console_client)
            .await
            .context("human_login is served but its console client is not usable")?;
        sms_auth::login::warm_up();
    }
    let sso = build_sso(db, config, &op_config.issuer).await?;
    let store = sms_auth::op::op_store(Arc::new(db.clone()), sys.clone());
    let state = op::OpState::new(
        store,
        signing,
        sms_auth::op::op_config(op_config.issuer.clone(), served),
        jwks,
    )
    .with_sso_login_endpoint(
        sso.as_ref()
            .map(|_| format!("{}{}", op_config.issuer, sms_auth::sso::START_PATH)),
    );
    // Keeps a rotate-signing-key run against this already-running process
    // effective without a restart.
    let verifier: Option<Arc<dyn BearerVerifier>> = realms(
        served,
        config
            .human_login
            .as_ref()
            .filter(|_| config.auth_methods.human_login_served()),
    )
    .map(|realms| {
        Arc::new(OpBearerVerifier::new(
            state.keyring(),
            Arc::new(DbKeys {
                db: db.clone(),
                sys: sys.clone(),
                issuer: op_config.issuer.clone(),
            }),
            &op_config.issuer,
            realms,
        )) as Arc<dyn BearerVerifier>
    });
    op::spawn_key_refresh(
        state.clone(),
        db.clone(),
        sys.clone(),
        op_config.issuer.clone(),
        std::time::Duration::from_secs(op_config.signing_key.refresh_interval_secs),
    );
    let limit =
        token_rate_limit::TokenRateLimitState::new(cratestack::ratelimit::RateLimitConfig::new(
            op_config.token_rate_limit.burst,
            op_config.token_rate_limit.refill_per_second,
        ));
    let mut router = op::router(state.clone(), limit, served);
    if let Some(human) = config
        .human_login
        .as_ref()
        .filter(|_| config.auth_methods.human_login_served())
    {
        router = router.merge(human_login_routes(db, state, human, sso, config.sso()));
    }
    info!(
        issuer = %op_config.issuer,
        grants = %served.grant_types().join(","),
        "the OpenID Provider is served: /token, /jwks.json and discovery are mounted"
    );
    if served.machine_tokens {
        info!("machine_tokens is served: /token mints tokens and the API accepts them as a bearer");
    }
    if served.human_login {
        info!(
            "human_login is served: POST /login signs a person in and /token redeems the code; \
             the API accepts the person's token; owner and admin act only from a paired console key"
        );
    }
    Ok(Some((router, verifier)))
}

/// `POST /login`, and the single-sign-on routes when `human_login.sso` is served
/// (`sso` is the provider `build_sso` made for `block`).
fn human_login_routes(
    db: &Cratestack,
    state: op::OpState,
    human: &sms_config::HumanLogin,
    sso: Option<sms_auth::sso::SsoProvider>,
    block: Option<&sms_config::Sso>,
) -> axum::Router {
    let mut router = login::router(
        db.clone(),
        state.clone(),
        &human.console_client.client_id,
        &human.console_client.redirect_uris,
        login::Limiter::new(&human.login_rate_limit),
    );
    if let (Some(provider), Some(block)) = (sso, block) {
        router = router.merge(sso::router(
            db.clone(),
            system_context("sms-gateway:sso"),
            state,
            provider,
            &human.console_client.client_id,
            &human.console_client.redirect_uris,
            block.rate_limit,
        ));
    }
    router
}

/// The single-sign-on broker, when `human_login.sso` is served; `Ok(None)` when it
/// is not.
///
/// **Nothing here may be fatal on a dead identity provider**: the provider is
/// reached lazily, on the first sign-in, so a gateway whose Keycloak is down
/// still starts and the password stays the break-glass way in. What is fatal is
/// what only an operator can fix and the gateway can check offline: a
/// `human_login.sso.jit_role` that names no `Role` row. Starts the transaction
/// reaper.
async fn build_sso(
    db: &Cratestack,
    config: &sms_config::Config,
    op_issuer: &str,
) -> Result<Option<sms_auth::sso::SsoProvider>> {
    let Some(block) = config.sso() else {
        return Ok(None);
    };
    let sys = system_context("sms-gateway:sso");
    let sso_config = sms_auth::sso::SsoConfig::new(block, op_issuer, config.deployment.livemode)
        .context("human_login.sso is not usable")?;
    if !sms_auth::sso::store::jit_role_exists(db, &sys, &block.jit_role)
        .await
        .context("checking human_login.sso.jit_role")?
    {
        anyhow::bail!(
            "human_login.sso.jit_role {:?} names no Role row: create the role (an owner can), \
             or name one of the built-in roles",
            block.jit_role
        );
    }
    info!(
        issuer = %sso_config.issuer,
        redirect_uri = %sso_config.redirect_uri,
        required_group = %sso_config.required_group,
        jit_role = %sso_config.jit_role_key,
        "single sign-on is served: register this redirect URI at the identity provider"
    );
    let provider = sms_auth::sso::SsoProvider::new(sso_config)
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("building the SSO provider")?;
    sso::spawn_transaction_reaper(db.clone(), sys);
    Ok(Some(provider))
}

/// The token realms the API accepts, from what is served: `None` when neither
/// is, so no bearer door opens at all.
///
/// No YAML key of its own: the person realm is served exactly when
/// `human_login` is, and it is addressed to the one console client that block
/// registers. `human` is the block, passed only when `human_login` is served.
fn realms(served: ServedGrants, human: Option<&sms_config::HumanLogin>) -> Option<Realms> {
    let person = human
        .filter(|_| served.human_login)
        .map(|block| Arc::from(block.console_client.client_id.as_str()));
    (served.machine_tokens || person.is_some()).then_some(Realms {
        machine: served.machine_tokens,
        person,
    })
}

/// The boot line for `auth.methods` (VSA-1): which methods are permitted, which
/// of them this release serves, and whether the key was written at all. Method
/// names only, never a key.
fn log_auth_methods(methods: &sms_config::AuthMethods) {
    let names = |iter: &mut dyn Iterator<Item = AuthMethod>| {
        iter.map(AuthMethod::as_str).collect::<Vec<_>>().join(",")
    };
    info!(
        permitted = %names(&mut methods.permitted()),
        served = %names(&mut methods.served()),
        explicit = methods.is_explicit(),
        "authentication methods (auth.methods; an absent key permits every method)"
    );
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
    use super::*;

    fn human(client_id: &str) -> sms_config::HumanLogin {
        let bucket = sms_config::Bucket {
            burst: 5,
            refill_per_second: 0.5,
        };
        sms_config::HumanLogin {
            console_client: sms_config::ConsoleClient {
                client_id: client_id.to_owned(),
                redirect_uris: vec!["https://console.example/cb".to_owned()],
            },
            login_rate_limit: sms_config::LoginRateLimit {
                per_email: bucket,
                aggregate: bucket,
            },
            sso: None,
        }
    }

    /// The table: for each combination of the grants served, the realms the
    /// verifier is built with, and whether a bearer door opens at all (which is
    /// what moves the envelope mode: `Required` without one, `Optional` with a
    /// keypair beside it, `Off` without a keypair). No YAML key decides any of
    /// it.
    #[test]
    fn the_realms_follow_the_grants_served_and_nothing_else() {
        let block = human("console_client_1");
        let console: Option<Arc<str>> = Some(Arc::from("console_client_1"));
        let served = |machine_tokens, human_login| ServedGrants {
            machine_tokens,
            human_login,
        };
        // (machine_tokens, human_login) -> realms
        assert_eq!(realms(served(false, false), None), None);
        assert_eq!(
            realms(served(true, false), None),
            Some(Realms {
                machine: true,
                person: None
            })
        );
        assert_eq!(
            realms(served(false, true), Some(&block)),
            Some(Realms {
                machine: false,
                person: console.clone()
            })
        );
        assert_eq!(
            realms(served(true, true), Some(&block)),
            Some(Realms {
                machine: true,
                person: console
            })
        );
        // The block alone does not serve the realm; `served` decides.
        assert_eq!(realms(served(false, false), Some(&block)), None);
        assert_eq!(
            realms(served(true, false), Some(&block)),
            Some(Realms {
                machine: true,
                person: None
            })
        );
    }

    /// The served sets through to the doors: a keypair beside a person realm
    /// opens the bearer door (so the envelope is `Optional`, not `Required` as it
    /// was before VSA-4b), and a person realm with neither the keypair nor
    /// `machine_tokens` is refused here even though the config refuses it first
    /// (`ConfigError::NoWayIn`): the doors layer backs it up.
    #[test]
    fn the_doors_follow_the_realms() {
        let block = human("console_client_1");
        let door = |keypair: bool, machine_tokens: bool, human_login: bool| {
            let served = ServedGrants {
                machine_tokens,
                human_login,
            };
            let bearer = realms(served, human_login.then_some(&block)).map(|_| {
                struct Never;
                #[async_trait::async_trait]
                impl BearerVerifier for Never {
                    async fn verify(
                        &self,
                        _token: &str,
                    ) -> Result<sms_api::VerifiedBearer, sms_api::BearerRefusal>
                    {
                        Err(sms_api::BearerRefusal::Malformed)
                    }
                }
                Arc::new(Never) as Arc<dyn BearerVerifier>
            });
            doors(keypair, served, bearer).map(|doors| (doors.keypair(), doors.bearer()))
        };
        // K, K+M, K+H, K+M+H, M, M+H
        assert_eq!(door(true, false, false), Ok((true, false)));
        assert_eq!(door(true, true, false), Ok((true, true)));
        assert_eq!(door(true, false, true), Ok((true, true)));
        assert_eq!(door(true, true, true), Ok((true, true)));
        assert_eq!(door(false, true, false), Ok((false, true)));
        assert_eq!(door(false, true, true), Ok((false, true)));
        // H alone: a person's token is not a way in by itself.
        assert_eq!(door(false, false, true), Err(sms_api::NoDoor));
        // Nothing served, nothing permitted.
        assert_eq!(door(false, false, false), Err(sms_api::NoDoor));
    }

    /// Not a test of a function in this file: the old defaults of the flags
    /// `serve` used to declare live in `sms_api`, and the base file must still
    /// state them. The same two numbers existed twice before VSC-1 (a clap
    /// default and an `sms_api` constant) and this holds the third copy, the
    /// YAML, to both.
    #[test]
    fn the_yaml_defaults_are_the_ones_sms_api_documents() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../config/application.yml");
        let config = sms_config::Config::load_with_env(
            Some(&base),
            sms_config::DEFAULT_PROFILE,
            sms_config::Binary::Gateway,
            &|name| (name == "SMS_HASH_PEPPER").then(|| "p".repeat(48)),
        )
        .expect("the committed base file loads");

        assert_eq!(
            std::time::Duration::from_secs(config.api.idempotency_ttl_secs),
            sms_api::DEFAULT_IDEMPOTENCY_TTL
        );
        let per_principal = sms_api::default_rate_limit_config();
        assert_eq!(config.api.rate_limit.burst, per_principal.burst);
        assert!(
            (config.api.rate_limit.refill_per_second - per_principal.refill_per_second).abs()
                < f64::EPSILON
        );
        let per_source = sms_api::default_source_rate_limit_config();
        assert_eq!(config.api.source_rate_limit.burst, per_source.burst);
        assert!(
            (config.api.source_rate_limit.refill_per_second - per_source.refill_per_second).abs()
                < f64::EPSILON
        );
    }

    /// A release build is not told to trust a fake certificate authority: the
    /// flag did not parse, and the YAML key is refused with the key's name.
    #[cfg(not(feature = "fake-aws"))]
    #[test]
    fn a_release_build_refuses_the_fake_certificate_key() {
        let aws = sms_config::AwsSms {
            sender: None,
            events_topic_arn: Some("arn:aws:sns:eu-west-1:123456789012:vsms-events".to_owned()),
            fake_sns_cert_base: Some("http://sms-fake-aws:8095/".to_owned()),
        };
        let error = super::aws_sms_receiver(Some(&aws), "dev")
            .err()
            .expect("refused");
        let message = error.to_string();
        assert!(message.contains("fake_sns_cert_base"), "{message}");
        assert!(
            message.contains("`dev` profile") && message.contains("SMS_PROFILE"),
            "{message}"
        );
    }

    #[cfg(not(feature = "fake-aws"))]
    #[test]
    fn a_receiver_without_the_fake_key_is_built() {
        let aws = sms_config::AwsSms {
            sender: None,
            events_topic_arn: Some("arn:aws:sns:eu-west-1:123456789012:vsms-events".to_owned()),
            fake_sns_cert_base: None,
        };
        assert!(
            super::aws_sms_receiver(Some(&aws), "dev")
                .ok()
                .flatten()
                .is_some()
        );
        assert!(
            super::aws_sms_receiver(None, "dev")
                .ok()
                .flatten()
                .is_none()
        );
    }

    /// The `fake-aws` build's receiver accepts a receipt `sms-fake-aws`
    /// signed, through the adapter's real verification.
    #[cfg(feature = "fake-aws")]
    #[tokio::test]
    async fn a_fake_aws_build_trusts_the_fakes_certificate_and_nothing_else_changes() {
        use sms_provider::{CallbackDisposition, RawCallback, SmsProvider};

        let _ = rustls::crypto::ring::default_provider().install_default();
        let fake = sms_fake_aws::FakeAws::start(
            sms_fake_aws::FakeAwsConfig::with_defaults("http://127.0.0.1:9/unused"),
            sms_fake_aws::FaultPolicy::always(sms_fake_aws::SubmitDecision::accepted_silently()),
        )
        .await;
        let receiver =
            super::fake_aws_receiver(Some(fake.topic_arn().to_owned()), fake.base_url()).unwrap();
        let raw = |body: String| RawCallback {
            headers: Vec::new(),
            body: body.into_bytes(),
        };
        let genuine = fake.signed_notification(r#"{"eventType":"TEXT_DELIVERED","messageId":"a"}"#);
        assert_eq!(
            receiver
                .authenticate_callback(&raw(genuine.clone()))
                .await
                .unwrap(),
            CallbackDisposition::Ingest
        );
        // Still the configured topic only.
        let other_topic = genuine.replace(
            fake.topic_arn(),
            "arn:aws:sns:eu-west-1:999999999999:attacker",
        );
        assert!(matches!(
            receiver
                .authenticate_callback(&raw(other_topic))
                .await
                .unwrap(),
            CallbackDisposition::Refused { .. }
        ));
        assert!(super::fake_aws_receiver(None, fake.base_url()).is_err());
    }
}
