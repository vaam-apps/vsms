//! `Command::Serve` — see that variant's own doc comment in `main.rs` for
//! the flags. This file is the whole `serve` binding path: the pool, the
//! signed-transport keys, the DLR router, both listeners, and graceful
//! shutdown.

use std::sync::Arc;

use anyhow::{Context, Result};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, provider as provider_filter};
use sms_config::{Binary, Providers};
use sms_provider::SmsProvider;
use tracing::info;

use crate::commands::server_keys;
use crate::{dlr, health};
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

    let dlr_router = build_dlr_router(&db, &sys, &config.providers, &args.config.profile).await?;
    // #157: /readyz needs the same pooled handle every other router
    // shares — cloned here, before `sms_api::router` takes `db` by value as
    // its own last use.
    let health_router = health::router(db.clone());

    let ServeArgs {
        listen,
        metrics_listen,
        ..
    } = args;
    let api = config.api;

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
    )
    .context("assembling the signed-transport router")?
    // Everything merged from here on sits OUTSIDE the envelope layer, which
    // `sms_api::router` applied to its own routes only: the provider
    // callbacks (a provider cannot sign) and the probes. See
    // `sms_api::router::router`'s doc. There is nothing else: no token
    // endpoint, no discovery document, no login.
    .merge(dlr_router)
    .merge(health_router);

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
