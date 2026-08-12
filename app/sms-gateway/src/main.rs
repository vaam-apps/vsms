//! The SMS gateway API server.

mod dlr;
mod health;
mod login;
mod op;
mod token_rate_limit;

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::FilterExpr;
use rand::Rng;
use sms_api::schema::procedures::{provision_app_client, ProcedureRegistry};
use sms_api::schema::{
    provider as provider_filter, route as route_filter, ClientAuthMethod, Cratestack,
    CreateOauthClientInput, CreateProviderInput, CreateRouteInput, CreateUserCredentialInput,
    CreateUserInput, ProviderKind, ProviderState, ProvisionClientInput, UpdateProviderInput,
    UpdateRouteInput,
};
use sms_api::{GatewayAuth, Principal, PrincipalKind, Procedures};
use sms_provider::SmsProvider;
use tracing::info;

/// Command-line surface.
#[derive(Debug, Parser)]
#[command(name = "sms-gateway", version, about = "A2P SMS gateway for Cameroon")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Bind the HTTP API.
    Serve {
        /// Address to listen on. Loopback by default: TLS terminates at a Caddy
        /// or nginx edge, and this process should never face the internet.
        #[arg(long, env = "SMS_LISTEN_ADDR", default_value = "127.0.0.1:8080")]
        listen: String,

        /// #70/#71: `GET /metrics`, Prometheus text exposition — bound to a
        /// **second, separate** listener, never merged into `--listen`'s own
        /// router. Loopback by default for the same reason `--listen`
        /// itself is: `deploy/Caddyfile`'s blanket `reverse_proxy
        /// sms-gateway:8080` never reaches this port at all, since it's a
        /// different port entirely — see `sms_api::metrics`'s own module
        /// doc for the full reasoning and `docs/runbooks/alerting.md` for
        /// how an operator points a real Prometheus at it.
        #[arg(
            long,
            env = "SMS_METRICS_LISTEN_ADDR",
            default_value = "127.0.0.1:9090"
        )]
        metrics_listen: String,

        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// Maximum pooled connections.
        #[arg(long, env = "SMS_DB_MAX_CONNECTIONS", default_value_t = 10)]
        max_connections: u32,

        /// The OP's own identity — every token this OP mints carries this
        /// as `iss`, and `GatewayAuth` validates incoming tokens against
        /// exactly this value. Never `listen` (a bind address, not an
        /// identity) — must be the externally reachable `https://` origin
        /// this OP is actually served at.
        #[arg(long, env = "SMS_OIDC_ISSUER")]
        issuer: String,

        /// `OAuth2` `client_credentials` client id for Orange Cameroon's
        /// SMS API — required unconditionally, unlike `sms-worker`'s own
        /// copy of this flag (optional there, only needed when `dispatch`
        /// is selected): this binary always serves the DLR route (#34),
        /// which always needs a provider to parse against.
        #[arg(long, env = "ORANGE_CM_CLIENT_ID")]
        orange_client_id: String,

        /// Paired with `orange_client_id`. Never logged.
        #[arg(long, env = "ORANGE_CM_CLIENT_SECRET")]
        orange_client_secret: String,

        /// E.164 without the `tel:` scheme.
        #[arg(long, env = "ORANGE_CM_SENDER_NUMBER")]
        orange_sender_number: String,

        /// Overridable so a real Orange sandbox (not just this crate's own
        /// `wiremock`-backed tests) can be pointed at without a code change.
        #[arg(
            long,
            env = "ORANGE_CM_BASE_URL",
            default_value = "https://api.orange.com"
        )]
        orange_base_url: String,

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
        hash_pepper: String,

        /// #153: how long a cached `Idempotency-Key` response stays
        /// replayable before a repeat with the same key is treated as a
        /// brand-new request. Matches `docs/architecture.md` §4.5's own
        /// figure (24h) as the default.
        #[arg(long, env = "SMS_IDEMPOTENCY_TTL_SECS", default_value_t = 24 * 60 * 60)]
        idempotency_ttl_secs: u64,

        /// #153: per-principal token-bucket capacity for
        /// `sms_api::router`'s `RateLimitLayer` — the burst a caller can
        /// spend before throttling kicks in. Matches §4.5's own suggested
        /// default; see `sms_api::default_rate_limit_config`'s doc for why
        /// that default is safe against this workspace's actual live-suite
        /// call volume. Distinct from `/token`'s own rate limiting, which
        /// §4.2 scopes to the reverse-proxy edge instead.
        #[arg(long, env = "SMS_RATE_LIMIT_BURST", default_value_t = 120)]
        rate_limit_burst: u32,

        /// #153: refill rate, in tokens/second, for the same bucket.
        #[arg(long, env = "SMS_RATE_LIMIT_REFILL_PER_SECOND", default_value_t = 2.0)]
        rate_limit_refill_per_second: f64,

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
        source_rate_limit_burst: u32,

        /// #163: refill rate, in tokens/second, for the same bucket.
        #[arg(
            long,
            env = "SMS_SOURCE_RATE_LIMIT_REFILL_PER_SECOND",
            default_value_t = 10.0
        )]
        source_rate_limit_refill_per_second: f64,

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
        token_rate_limit_burst: u32,

        /// #168: refill rate, in tokens/second, for the same bucket — 20
        /// events/minute.
        #[arg(
            long,
            env = "SMS_TOKEN_RATE_LIMIT_REFILL_PER_SECOND",
            default_value_t = token_rate_limit::default_token_rate_limit_config().refill_per_second
        )]
        token_rate_limit_refill_per_second: f64,

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
        console_client_id: String,
    },
    /// Print the generated route table and exit. Needs no database.
    Routes,
    /// Generate a new RSA signing key, activate it, and keep the previous
    /// one publishing in JWKS for `sms_auth::op::ROTATION_OVERLAP` — an
    /// operator action, not a generated-CRUD route (`OauthSigningKey`'s own
    /// schema comment: this is the key that signs every token the OP
    /// issues, and it must never be reachable except as `hasRole('system')`
    /// already restricts it to).
    RotateSigningKey {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
    },
    /// Mint a real, HTTP-usable `private_key_jwt` client through the real
    /// `provisionAppClient` procedure — an operator action, not a
    /// generated-CRUD route, for the identical reason `RotateSigningKey`
    /// above is one: `provisionAppClient`'s own `@allow` in
    /// `schema.cstack` is `hasRole('owner') || hasRole('admin')`, and
    /// `GatewayAuth::authenticate` never mints either role for a real
    /// token (no human-login flow exists yet — see `AGENTS.md`'s M1
    /// section). So nothing this deployment can issue over HTTP can call
    /// it; this subcommand calls `Procedures::provision_app_client`
    /// directly, under a hand-built `owner`/`admin` context, the same way
    /// `app/sms-gateway/tests/m1_acceptance_gate_live_postgres.rs` already
    /// does for its own acceptance gate. See #137.
    ///
    /// `ProvisionClientResult` returns `privateKeyPem` exactly once and it
    /// is never stored anywhere in this system (#23/#111) — this command
    /// writes it straight to `--key-out` with `0600` permissions and
    /// refuses to overwrite an existing file, and it is never logged or
    /// printed alongside anything else.
    ProvisionClient {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// The `App.id` this client acts on behalf of. Must already exist
        /// and be active — `provision_client` checks both and refuses
        /// otherwise.
        #[arg(long)]
        app_id: String,

        /// A human-readable label for the resulting `AppClient`, e.g.
        /// `"admin console"` or `"otp sender"`.
        #[arg(long)]
        label: String,

        /// One or more scopes to provision the client with, e.g.
        /// `--scope sms:send --scope sms:read`. At least one is required —
        /// an unscoped client can authenticate but can call nothing.
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,

        /// Which of `provisionAppClient`'s two admitted roles to run the
        /// call under. Both are equally privileged for this call; `owner`
        /// is the default because it's the role every existing live test
        /// already provisions under (`m1_acceptance_gate_live_postgres.rs`,
        /// `provision_app_client_live_postgres.rs`).
        #[arg(long, default_value = "owner")]
        role: String,

        /// Where to write the returned private key, PEM-encoded. Created
        /// with `0600` permissions; this command refuses to run if the
        /// path already exists rather than silently overwriting a key
        /// someone may still be using.
        #[arg(long)]
        key_out: PathBuf,

        /// #134: `Procedures::new` now requires a `HashPepper` unconditionally,
        /// even though `provision_app_client` itself never hashes anything —
        /// only `sendMessage` does. Same flag name and env var as `Serve`'s
        /// own `--hash-pepper`/`SMS_HASH_PEPPER`, so an operator running
        /// this alongside `serve` supplies the identical value once via
        /// their environment rather than learning two different names for
        /// the same secret.
        #[arg(long, env = "SMS_HASH_PEPPER")]
        hash_pepper: String,
    },
    /// Seed (or reactivate) the `Provider` row `resolve_provider_row_id`
    /// (this file) resolves at startup, **and** a catch-all `Route`
    /// pointing at it — an operator action, not a generated-CRUD route,
    /// for the same reason `RotateSigningKey`/`ProvisionClient` above are
    /// one: `Provider`'s own `@allow` in `schema.cstack` admits only
    /// `hasRole('owner') || hasRole('admin')` on create (`Route`'s is
    /// identical), and `GatewayAuth::authenticate` never mints either for
    /// a real token (no human-login flow exists yet — see `AGENTS.md`'s
    /// M1 section).
    ///
    /// See #148: nothing in this repo seeded the `Provider` row before
    /// this command existed, and `resolve_provider_row_id` fails at
    /// process start — not lazily — the moment `serve` runs against a
    /// fresh database, so a Helm install with no window for manual
    /// intervention between the pre-install hooks completing and the
    /// gateway `Deployment` being created crash-looped forever.
    ///
    /// **Renamed from `SeedProvider` (`seed-provider`) to `SeedDispatch`
    /// (`seed-dispatch`) and extended to also seed a `Route`, found live
    /// while closing out #62's own PR review — a real "documentation
    /// asserts something the code does not do" gap, the fifth instance
    /// `AGENTS.md` records: #62's routing engine made `dispatch` refuse
    /// every message with no matching `Route` (a deliberate cutover from
    /// the old "any active provider" placeholder), but this command —
    /// which both deployment runbooks tell an operator to run instead of
    /// `send_test_message` — only ever created the `Provider` row. A
    /// deployment that followed either runbook via this command reached
    /// `sms-gateway` healthy and `sms-worker` running, with every message
    /// silently landing in `rejected` forever. The old name no longer
    /// described what running it actually leaves you able to do — a
    /// deployment with only a `Provider` row cannot route anything to
    /// it — so this is a hard rename, not an alias, matching this repo's
    /// own standing "hard cutover, not a parallel/back-compat path"
    /// preference; every reference (`deploy/docker-compose.yml`,
    /// `deploy/charts/vsms/values.yaml`, both runbooks) is updated in the
    /// same change.
    ///
    /// The `Route` half is a hardcoded catch-all — no flags to configure
    /// `priority`/`weight`/`match*`, matching `send_test_message.rs`'s own
    /// `ensure_route` (same fixture-quality scope: "make this deployment
    /// able to send something," not a real routing policy authoring tool,
    /// which is #54's job). Idempotent by construction, the same
    /// "existing state is success, not failure" discipline the `Provider`
    /// half already used: the `Provider` half is `create` + catch the
    /// `23505` on `Provider.key`'s `@unique` index (this repo's documented
    /// dedupe pattern — `upsert` doesn't exist when the `@id` carries a
    /// default); `Route` has no unique column to catch a conflict on, so
    /// its half is find-by-`providerId`-then-create instead (re-enabling a
    /// disabled leftover route rather than adding a second one) — see
    /// [`ensure_catch_all_route`]'s own doc for the small TOCTOU window
    /// that shape accepts and why it's fine for an idempotent ops command.
    /// Either way, a Helm `pre-install`/`pre-upgrade` hook can run this on
    /// every install and upgrade without erroring or duplicating on the
    /// second run.
    SeedDispatch {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// Must match `SmsProvider::key()` for whichever adapter is
        /// actually configured — `resolve_provider_row_id` looks the row
        /// up by exactly this key. `orange_cm` is the only adapter with a
        /// real implementation (`sms-provider-orange-cm`) as of this
        /// milestone, matching `Serve`'s own hard-coded Orange wiring.
        #[arg(long, default_value = "orange_cm")]
        key: String,

        #[arg(long, default_value = "Orange Cameroon SMS API")]
        display_name: String,

        /// One of `ProviderKind`'s own variants (`schema.cstack`):
        /// `orange_cm_http`, `mtn_http`, `aggregator_http`, `smpp`.
        #[arg(long, default_value = "orange_cm_http")]
        kind: String,

        /// Never read by `sms-gateway` or `sms-worker` to construct the
        /// real adapter — both build it from their own flags/env instead
        /// (§2.4), confirmed against `send_test_message.rs`'s own doc
        /// comment. This row's job is only to exist, carry the right
        /// `key`, and end up `state = 'active'`.
        #[arg(long, default_value = "{}")]
        config: String,

        #[arg(long, default_value = "env:ORANGE_CM_CLIENT_SECRET")]
        credential_ref: String,

        #[arg(long, default_value_t = 10.0)]
        max_tps: f64,

        #[arg(long, default_value_t = 100_000)]
        max_daily_submissions: i64,

        /// Parsed as a `cratestack::Decimal`, not a float — money stays
        /// fixed-point throughout this codebase (§2.0).
        #[arg(long, default_value = "0")]
        cost_per_segment_xaf: String,

        /// Which of `Provider`'s two create-admitted roles to run this
        /// call under. Same choice, same reasoning as `ProvisionClient`'s
        /// own `--role`: `owner` is the default because every existing
        /// live Provider fixture in this repo already writes under it.
        #[arg(long, default_value = "owner")]
        role: String,
    },
    /// Registers the `sms-console` `OauthClient` row #194's human login
    /// flow needs — an operator action, not a generated-CRUD route, for
    /// the identical reason `SeedProvider` above is one: `OauthClient`'s
    /// own `@allow` in `schema.cstack` is `hasRole('system')` only, and
    /// `GatewayAuth::authenticate` never mints that role for any real
    /// token. Public client (`token_endpoint_auth_method = none`): the
    /// console is a first-party BFF whose `redirect_uri` and mandatory
    /// PKCE are the protection, and no column in this schema could hold a
    /// shared secret it would need one for (§2.2, the same reasoning
    /// `private_key_jwt` service accounts already rely on).
    ///
    /// Idempotent: a `23505` on `clientId`'s `@unique` index (this
    /// row already existing from an earlier run) is treated as success,
    /// matching `SeedProvider`'s own convention — safe to run on every
    /// deploy.
    SeedConsoleClient {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        /// Must match `sms-gateway serve --console-client-id` and
        /// `admin`'s own `SMS_CONSOLE_OIDC_CLIENT_ID` exactly —
        /// `GatewayAuth`'s human-token audience check
        /// (`sms_api::auth::GatewayAuth`'s own doc) refuses any other
        /// value outright.
        #[arg(long, env = "SMS_CONSOLE_OIDC_CLIENT_ID", default_value = sms_api::DEFAULT_CONSOLE_CLIENT_ID)]
        client_id: String,

        /// The exact, single `redirect_uri` this client is registered
        /// with — `authkestra_op::handlers::authorize::handle_authorize`
        /// requires an exact string match (RFC 6749 §3.1.2), not a prefix
        /// or origin match. Must equal `{ADMIN_BASE_URL}/api/auth/callback`
        /// from `admin`'s own `@vsms/env` schema.
        #[arg(long)]
        redirect_uri: String,
    },
    /// Creates a `User` + `UserCredential` for #194's human login flow —
    /// an operator action, not a generated-CRUD route, for the identical
    /// reason `ProvisionClient` above is one: `User`'s own `@allow` in
    /// `schema.cstack` admits `hasRole('owner') || hasRole('admin')` on
    /// create, and no real token this deployment issues can ever carry
    /// either (the same gap this whole ticket exists to close — a human
    /// has to be able to log in before one can provision another human,
    /// so the very first account is necessarily bootstrapped this way).
    ///
    /// Generates a random password, hashes it with the real
    /// `sms_auth::login::hash_password` (Argon2id — never a weaker
    /// scheme just because this is a CLI tool), and prints the plaintext
    /// exactly once — the same "returned once, never stored, never
    /// logged" discipline `ProvisionClient`'s own `privateKeyPem` already
    /// follows, applied here because there is no operator-supplied
    /// `--password` flag: a CLI argument would land in shell history and
    /// the process list, exactly the exposure `write_private_key_pem`'s
    /// own `--key-out` file exists to avoid for the client-provisioning
    /// case.
    ProvisionUser {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,

        #[arg(long)]
        email: String,

        #[arg(long)]
        display_name: String,

        /// Must already exist — `User.roleKey` is a foreign key to
        /// `Role.key`, and this command does not create roles (§5.2's
        /// built-in roles are not seeded by any migration; an `owner`
        /// account has to exist to create the first `Role` row through
        /// the generated API, or one is inserted by hand against a fresh
        /// database — see the deploy runbook).
        #[arg(long)]
        role_key: String,
    },
    /// Exec-form liveness/readiness check for orchestrators that can't run
    /// a shell — a distroless `static` runtime image (see
    /// `app/sms-gateway/Dockerfile`) has no `/bin/sh` and no `curl`, so
    /// neither the container `HEALTHCHECK` nor a Compose/Kubernetes exec
    /// probe can shell out the way they used to. This does the identical
    /// check the old `curl -fsS http://<addr><path>` did — a plain HTTP/1.1
    /// GET over a raw socket, no TLS, no dependency beyond `std` — and
    /// exits non-zero (via `Result`'s `Err` under `#[tokio::main]`) on
    /// anything but a `200`. Defaults match `health.rs`'s own `/healthz`;
    /// pass `--path /readyz` for the readiness variant
    /// `deploy/docker-compose.yml`/`deploy/charts/vsms/values.yaml` want
    /// instead (the Helm chart's own `readinessProbe` stays a native
    /// Kubernetes `httpGet` probe, which needs no in-container shell at
    /// all — this subcommand exists for the two places that do).
    Healthcheck {
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,

        #[arg(long, default_value = "/healthz")]
        path: String,
    },
}

/// Writes `pem` to a freshly created file at `path`, `0600` on Unix,
/// refusing to overwrite an existing file (`O_EXCL` via
/// [`std::fs::OpenOptions::create_new`]) — both the mode and the
/// exclusivity are applied atomically at `open(2)` time, so there is no
/// window where the file exists with looser permissions or already-visible
/// contents. See `Command::ProvisionClient`'s own doc comment for why this
/// exists: `ProvisionClientResult::privateKeyPem` is returned exactly once
/// and this is the only place in this system it is ever persisted.
fn write_private_key_pem(path: &std::path::Path, pem: &str) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| {
        format!(
            "creating {} — refusing to overwrite an existing file, since it may hold a private \
             key still in use",
            path.display()
        )
    })?;
    file.write_all(pem.as_bytes())
        .with_context(|| format!("writing the private key to {}", path.display()))?;
    file.flush()
        .with_context(|| format!("flushing the private key to {}", path.display()))
}

/// The `system`-role context every OP-adjacent database write in this
/// binary runs under — never handed to a caller, matching
/// `Procedures::sys()`'s own convention.
fn system_context() -> cratestack::CoolContext {
    Principal {
        sub: "sms-gateway:op".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
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
    sys: &cratestack::CoolContext,
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

#[tokio::main]
async fn main() -> Result<()> {
    // Must run before anything constructs an HTTP client — see
    // `install_default_crypto_provider`'s own doc for why.
    install_default_crypto_provider();

    // Variables already in the environment win; dotenvy never overwrites.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sms_gateway=info,sms_api=info,cratestack=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Routes => {
            let routes = sms_api::route_table();
            println!("{} generated routes:", routes.len());
            for (method, path) in routes {
                println!("  {method:<7} {path}");
            }
            Ok(())
        }

        command @ Command::Serve { .. } => serve_command(command).await,

        Command::RotateSigningKey { database_url } => {
            rotate_signing_key_command(database_url).await
        }

        command @ Command::ProvisionClient { .. } => provision_client_command(command).await,

        command @ Command::SeedDispatch { .. } => seed_dispatch_command(command).await,

        command @ Command::SeedConsoleClient { .. } => seed_console_client_command(command).await,

        command @ Command::ProvisionUser { .. } => provision_user_command(command).await,

        Command::Healthcheck { addr, path } => healthcheck_command(&addr, &path),
    }
}

/// Installs `ring` as the process-wide default `rustls` `CryptoProvider`.
///
/// Load-bearing, not defensive: `authkestra-op`/`-engine`/`-resource`/`-axum`
/// are pinned with `default-features = false, features =
/// ["rustls-no-provider"]` (Cargo.toml's own comment on those pins), which
/// drops `aws-lc-rs` out of the dependency graph entirely — that crate's
/// `cmake`/pkg-config build requirement is exactly what made a musl build
/// impractical before. But it also means authkestra's own `reqwest` client
/// carries no crypto backend baked in at all; the first TLS handshake it
/// attempts panics unless *something* has already called
/// `CryptoProvider::install_default()` for the whole process. `reqwest`
/// 0.12 elsewhere in this binary's dependency tree (via `sms-api`,
/// `sms-provider-orange-cm`) still resolves `ring` through its own
/// `rustls-tls` feature and would install it lazily on first use — but
/// relying on *some other* client happening to be built first is exactly
/// the "unverified runtime path" AGENTS.md warned off; this makes the
/// order explicit and unconditional instead. `ring`, not `aws-lc-rs`, to
/// match every other TLS consumer already in this tree (AGENTS.md: "the
/// whole cratestack family selects ring").
///
/// `.ok()`, not `.expect(...)`: the only failure mode is a provider already
/// installed (impossible this early, since this is the first line of
/// `main`, but not worth a panic if that ever changes) — never a reason to
/// abort startup.
fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// `Command::Healthcheck`'s body — see that variant's own doc comment for
/// why this exists at all (a distroless `static` image has no shell or
/// `curl` for the container/orchestrator health check to shell out to) and
/// why it's a hand-rolled HTTP/1.1 GET rather than pulling in `reqwest`:
/// this only ever needs to run against `127.0.0.1`, in-process, so a raw
/// socket is simpler than standing up a TLS-capable client for a plaintext
/// loopback request.
fn healthcheck_command(addr: &str, path: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let mut stream = TcpStream::connect(addr)
        .with_context(|| format!("connecting to {addr} for healthcheck"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("setting healthcheck read timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .context("setting healthcheck write timeout")?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .context("writing healthcheck request")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("reading healthcheck response")?;
    let status_line = response
        .lines()
        .next()
        .context("empty healthcheck response")?;
    // e.g. "HTTP/1.1 200 OK" — the status code is always the second
    // whitespace-delimited token of the status line (RFC 9112 §4).
    if status_line.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        bail!("unhealthy: GET {addr}{path} returned {status_line:?}")
    }
}

/// `Command::Serve`'s body, pulled out of `main`'s own `match` for the same
/// `clippy::too_many_lines` reason [`rotate_signing_key_command`] below
/// already was — #168 pushed `main` back over the limit by adding two more
/// CLI flags and the `token_rate_limit` wiring they feed, the same shape of
/// growth #139 caused originally. Takes the whole matched `Command` (rather
/// than its dozen-plus fields individually) for the identical reason
/// [`provision_client_command`] does — see that function's own doc; the
/// `unreachable!()` below can never fire because the only caller is
/// `main`'s own `command @ Command::Serve { .. }` guard.
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
    sys: &cratestack::CoolContext,
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
    sys: &cratestack::CoolContext,
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

async fn serve_command(command: Command) -> Result<()> {
    let Command::Serve {
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
    } = command
    else {
        unreachable!("only ever called with Command::Serve")
    };

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
    let sys = system_context();

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

/// `Command::RotateSigningKey`'s body, pulled out of `main`'s own `match`
/// for the same reason [`provision_client_command`] below is — see that
/// function's doc.
async fn rotate_signing_key_command(database_url: String) -> Result<()> {
    // Found live prepping #36 (this repo's first time ever running
    // this command against a real database): `max_connections(1)`
    // deadlocks with `CoolError::Database("pool timed out waiting
    // for an open connection")` on the very first
    // `OauthSigningKey::create` — reproduced twice, and confirmed
    // fixed at exactly `max_connections(2)`, one shy of that. Most
    // likely cause: `OauthSigningKey`'s `@@audit` writes its audit
    // row on a connection acquired separately from (and held
    // concurrently with) the row write's own, rather than sharing
    // one — unconfirmed against `cratestack`'s own generated code,
    // but the connection-count boundary itself is empirically
    // solid. A one-shot CLI command needs no real concurrency, so
    // this stays small — a modest margin above the confirmed
    // minimum, not `serve`'s pooled-server-sized default, in case
    // a rotation with several still-active-but-expiring keys to
    // deactivate needs more than one such pair in flight.
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let id =
        sms_auth::op::rotate_signing_key(&db, &system_context(), sms_auth::op::ROTATION_OVERLAP)
            .await
            .context("rotating the OP signing key")?;
    println!("rotated: new signing key {id} is now active");
    println!(
        "the previous key keeps publishing in JWKS for {} minutes",
        sms_auth::op::ROTATION_OVERLAP.num_minutes()
    );
    Ok(())
}

/// `Command::ProvisionClient`'s body, pulled out of `main`'s own `match`
/// purely to stay under `clippy::too_many_lines` — see that variant's own
/// doc comment for what this does and why it exists. Takes the whole
/// matched `Command` (rather than its fields individually) so the
/// multi-line destructure lives here instead of adding more lines to the
/// already-large `match` in `main`; the `unreachable!()` below can never
/// fire because the only caller is `main`'s own `command @
/// Command::ProvisionClient { .. }` guard.
async fn provision_client_command(command: Command) -> Result<()> {
    let Command::ProvisionClient {
        database_url,
        app_id,
        label,
        scopes,
        role,
        key_out,
        hash_pepper,
    } = command
    else {
        unreachable!("only ever called with Command::ProvisionClient")
    };

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — provisionAppClient's own @allow admits \
             nothing else, got {role:?}"
        );
    }
    // Refuse up front, before touching the database at all, so a typo'd
    // --key-out never causes a real provisioning call (and a real,
    // now-orphaned private key) that this process then fails to hand back
    // to the operator.
    if key_out.exists() {
        bail!(
            "{} already exists — refusing to overwrite a file that may hold a private key \
             still in use; pass a different --key-out",
            key_out.display()
        );
    }
    // #134: validated up front for the same reason `Serve` validates its
    // own copy before doing anything else — `provision_app_client` never
    // hashes anything itself, but `Procedures::new` takes an unconditional
    // `HashPepper` regardless, so a bad pepper must fail before a real
    // provisioning call happens, not after.
    let pepper = sms_api::HashPepper::new(hash_pepper)
        .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

    // Same conservative pool size as `RotateSigningKey`, and for the same
    // reason: this is a one-shot CLI command writing two `@@audit`-backed
    // rows (`AppClient`, `OauthClient`) in one transaction, the same shape
    // of write that command's own comment found deadlocks at
    // `max_connections(1)`. Never empirically re-tested at 1 here, so the
    // same modest margin is kept rather than assumed safe at the minimum.
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let ctx = Principal {
        sub: format!("sms-gateway:provision-client:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let procedures = Procedures::new(pepper);
    let provisioned = procedures
        .provision_app_client(
            &db,
            &ctx,
            provision_app_client::Args {
                args: ProvisionClientInput {
                    appId: app_id,
                    label,
                    scopes,
                },
            },
        )
        .await
        .context("provisioning the client")?;

    // Destructured immediately and never reassembled: nothing past this
    // point may hold, log, or `{:?}`-print `provisioned` as a whole — see
    // `write_private_key_pem`'s own doc for why the file below is the
    // only place this value's private key is ever allowed to land.
    let client_id = provisioned.clientId;
    let private_key_pem = provisioned.privateKeyPem;

    write_private_key_pem(&key_out, &private_key_pem)?;

    println!("provisioned client: {client_id}");
    println!("private key written to: {}", key_out.display());
    println!();
    println!("paste into the console (or any other machine caller)'s environment:");
    println!("  SMS_CONSOLE_CLIENT_ID={client_id}");
    println!("  SMS_CONSOLE_PRIVATE_KEY_PATH={}", key_out.display());
    Ok(())
}

/// `ProviderKind`'s variants aren't `clap::ValueEnum` (it's a type generated
/// by `include_server_schema!` in a downstream crate, not one this binary
/// can derive a foreign trait on), so `--kind` stays a plain `String` and is
/// matched by hand here — pulled out of [`seed_dispatch_command`] purely to
/// keep that function under `clippy::too_many_lines`, the same reason
/// [`rotate_signing_key_command`]/[`provision_client_command`] were already
/// split out of `main`'s own `match`.
fn parse_provider_kind(kind: &str) -> Result<ProviderKind> {
    match kind {
        "orange_cm_http" => Ok(ProviderKind::orange_cm_http),
        "mtn_http" => Ok(ProviderKind::mtn_http),
        "aggregator_http" => Ok(ProviderKind::aggregator_http),
        "smpp" => Ok(ProviderKind::smpp),
        other => bail!(
            "--kind {other:?} is not a ProviderKind variant — one of orange_cm_http, mtn_http, \
             aggregator_http, smpp"
        ),
    }
}

/// `create` the `Provider` row, or resolve the id of the one that already
/// exists — pulled out of [`seed_dispatch_command`] purely to keep that
/// function under `clippy::too_many_lines`.
///
/// A `23505` on `Provider.key`'s `@unique` index means some earlier run
/// already created this row — a fresh install's `pre-install` hook and a
/// later `pre-upgrade` hook both invoke this exact command, and Helm itself
/// may retry a hook Job that failed for an unrelated reason — so that case
/// falls back to looking the row up by key rather than treating the
/// conflict as a failure. Returns the row id, its current `@version` (#59 —
/// `Provider` is now versioned, and the caller's own follow-up activation
/// write needs `if_match`), and whether it is already `state = 'active'`,
/// so the caller can skip a needless activation write.
async fn create_or_find_provider(
    db: &Cratestack,
    ctx: &cratestack::CoolContext,
    key: &str,
    input: CreateProviderInput,
) -> Result<(String, i64, bool)> {
    match db.provider().create(input).run(ctx).await {
        Ok(created) => {
            println!("created Provider {} (key={key:?})", created.id);
            Ok((created.id, created.version, false))
        }
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {
            let existing = db
                .provider()
                .find_many()
                .where_expr(FilterExpr::from(provider_filter::key().eq(key.to_owned())))
                .limit(1)
                .run(ctx)
                .await
                .context("looking up the existing Provider row after a duplicate-key create")?;
            let row = existing.into_iter().next().with_context(|| {
                format!(
                    "Provider row with key {key:?} reported as a duplicate on create but not \
                     found on lookup"
                )
            })?;
            println!(
                "Provider {} (key={key:?}) already exists — state={:?}",
                row.id, row.state
            );
            Ok((row.id, row.version, row.state == ProviderState::active))
        }
        Err(e) => Err(e).context("creating the Provider row"),
    }
}

/// Ensure a `Route` row points at `provider_id`, creating a hardcoded
/// catch-all (`priority: 0, weight: 1`, every `match*` a wildcard) if none
/// exists yet — pulled out of [`seed_dispatch_command`] for the same
/// `clippy::too_many_lines` reason [`create_or_find_provider`] was.
///
/// `Route` carries no unique column the way `Provider.key` does, so this
/// can't use `create` + catch-`23505` — it looks up an existing route for
/// this provider first and only creates one if none is found. That is a
/// real, accepted TOCTOU window (two concurrent runs of this command
/// against a never-before-seeded database could both find nothing and
/// both create a route), narrower in practice than it sounds: this
/// command is invoked from a Helm `pre-install`/`pre-upgrade` hook, whose
/// own `Job` semantics don't run two instances of the same hook
/// concurrently, and a `docker compose run --rm` invocation is a manual,
/// one-at-a-time operator action. A duplicate catch-all route would be
/// harmless in any case — #62's routing engine already treats a tie
/// between two equal-priority, equal-weight wildcard routes as an
/// ordinary weighted draw, not a correctness bug — but it's worth
/// contrasting with `create_or_find_provider`'s stronger, constraint-backed
/// guarantee rather than silently assuming the same guarantee applies
/// here.
async fn ensure_catch_all_route(
    db: &Cratestack,
    ctx: &cratestack::CoolContext,
    provider_id: &str,
) -> Result<()> {
    let existing = db
        .route()
        .find_many()
        .where_expr(FilterExpr::from(
            route_filter::providerId().eq(provider_id.to_owned()),
        ))
        .limit(1)
        .run(ctx)
        .await
        .context("looking up an existing Route for this provider")?;

    if let Some(row) = existing.into_iter().next() {
        if row.enabled {
            println!(
                "Route {} already exists and is enabled (provider={provider_id})",
                row.id
            );
        } else {
            db.route()
                .update(row.id.clone())
                .set(UpdateRouteInput {
                    enabled: Some(true),
                    ..Default::default()
                })
                // #59: Route is @version'd. Runtime-enforced, not
                // compile-enforced — without this, `seed-dispatch` (the
                // command both runbooks tell an operator to run) fails at
                // exactly the point it is meant to repair a disabled route.
                .if_match(row.version)
                .run(ctx)
                .await
                .context("re-enabling the existing Route")?;
            println!(
                "Route {} (provider={provider_id}) was disabled — re-enabled it",
                row.id
            );
        }
        return Ok(());
    }

    let created = db
        .route()
        .create(CreateRouteInput {
            name: "catch-all (seed-dispatch)".to_owned(),
            priority: 0,
            weight: 1,
            enabled: true,
            matchOperator: None,
            matchClass: None,
            matchAppId: None,
            matchPrefix: None,
            providerId: provider_id.to_owned(),
            failoverRouteId: None,
        })
        .run(ctx)
        .await
        .context("creating a catch-all Route")?;
    println!(
        "created catch-all Route {} (provider={provider_id})",
        created.id
    );
    Ok(())
}

/// `Command::SeedDispatch`'s body, pulled out of `main`'s own `match` for
/// the same reason `rotate_signing_key_command`/`provision_client_command`
/// above are.
///
/// Idempotent: [`create_or_find_provider`] treats an already-existing row
/// as success rather than failure, and the row is left `state = 'active'`
/// either way — a freshly created row always starts `disabled`
/// (`Provider.state`'s own `@default`) so it is unconditionally activated,
/// but an *existing* row is only re-activated if it isn't already, so the
/// steady-state case this command exists for (a `pre-upgrade` hook
/// re-running against an already-seeded database) writes nothing on the
/// `Provider` half at all, rather than bumping `updatedAt` and appending
/// an `@@audit` row on every single upgrade for no behavioural change.
/// [`ensure_catch_all_route`] always runs, regardless of whether the
/// `Provider` half was already active — found live while fixing #62's own
/// gap: an earlier draft of this function returned early on
/// `already_active` *before* the `Route` half ran at all, which would
/// have left every re-run against an already-active `Provider` (the
/// actual steady-state case a `pre-upgrade` hook hits on every single
/// upgrade) never checking whether a `Route` exists.
async fn seed_dispatch_command(command: Command) -> Result<()> {
    let Command::SeedDispatch {
        database_url,
        key,
        display_name,
        kind,
        config,
        credential_ref,
        max_tps,
        max_daily_submissions,
        cost_per_segment_xaf,
        role,
    } = command
    else {
        unreachable!("only ever called with Command::SeedDispatch")
    };

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — Provider's own @allow admits nothing else \
             on create, got {role:?}"
        );
    }

    let kind = parse_provider_kind(&kind)?;
    let cost_per_segment_xaf: cratestack::Decimal = cost_per_segment_xaf
        .parse()
        .context("--cost-per-segment-xaf must parse as a decimal")?;

    // Same conservative pool size as RotateSigningKey/ProvisionClient, and
    // for the same reason: this is a one-shot CLI command writing a
    // handful of @@audit-backed rows (Provider, Route), the same shape of
    // write that rotate_signing_key_command's own comment found deadlocks
    // at max_connections(1).
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let ctx = Principal {
        sub: format!("sms-gateway:seed-dispatch:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let (provider_id, provider_version, already_active) = create_or_find_provider(
        &db,
        &ctx,
        &key,
        CreateProviderInput {
            key: key.clone(),
            displayName: display_name,
            kind,
            config,
            credentialRef: credential_ref,
            maxTps: max_tps,
            maxDailySubmissions: max_daily_submissions,
            // Not read by either binary to construct the real adapter
            // (see this variant's own doc comment) and not yet consulted
            // by dispatch's routing pass either — placeholders, same as
            // `config`/`credentialRef` above, matching every existing
            // Provider fixture in this repo's live test suites.
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: cost_per_segment_xaf,
            healthCheckedAt: None,
        },
    )
    .await?;

    if already_active {
        println!("Provider already active — nothing to do there");
    } else {
        db.provider()
            .update(provider_id.clone())
            .set(UpdateProviderInput {
                state: Some(ProviderState::active),
                ..Default::default()
            })
            // #59: Provider is @version'd now. Nothing else in this
            // one-shot seeding command can race this write, but the
            // framework refuses a versioned-model update without an
            // If-Match at runtime — it is not a compile-time error, so
            // a missing one surfaces only when the command is actually
            // run.
            .if_match(provider_version)
            .run(&ctx)
            .await
            .context("activating the Provider row")?;
        println!("activated Provider {provider_id} (key={key:?})");
    }

    ensure_catch_all_route(&db, &ctx, &provider_id).await
}

/// `Command::SeedConsoleClient`'s body (#194) — see that variant's own doc
/// comment for what this does and why. Idempotent, same
/// create-then-catch-23505 shape as [`create_or_find_provider`].
async fn seed_console_client_command(command: Command) -> Result<()> {
    let Command::SeedConsoleClient {
        database_url,
        client_id,
        redirect_uri,
    } = command
    else {
        unreachable!("only ever called with Command::SeedConsoleClient")
    };

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let sys = system_context();

    let input = CreateOauthClientInput {
        clientId: client_id.clone(),
        appClientId: None,
        tokenEndpointAuthMethod: ClientAuthMethod::none,
        jwks: None,
        grantTypes: " authorization_code refresh_token ".to_owned(),
        scopes: " openid profile ".to_owned(),
        redirectUris: format!(" {redirect_uri} "),
        requirePkce: true,
    };

    match db.oauth_client().create(input).run(&sys).await {
        Ok(created) => {
            println!(
                "registered sms-console OauthClient {} (clientId={client_id:?})",
                created.id
            );
        }
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {
            println!(
                "sms-console OauthClient with clientId={client_id:?} already exists — nothing \
                 to do (redirect_uri is not updated on an existing row by this command; re-run \
                 by hand against the database if it needs to change)"
            );
        }
        Err(e) => return Err(e).context("seeding the sms-console OauthClient row"),
    }
    Ok(())
}

/// `Command::ProvisionUser`'s body (#194) — see that variant's own doc
/// comment for what this does, why it exists, and why the password is
/// generated rather than accepted as a flag.
async fn provision_user_command(command: Command) -> Result<()> {
    let Command::ProvisionUser {
        database_url,
        email,
        display_name,
        role_key,
    } = command
    else {
        unreachable!("only ever called with Command::ProvisionUser")
    };

    // 24 alphanumeric characters is ~142 bits of entropy — comfortably
    // more than Argon2id's own hashing cost is meant to protect against a
    // brute-force guess of, and short enough an operator can read it over
    // a phone call for a break-glass first account. rand::thread_rng(),
    // not a hand-rolled PRNG — the same source rsa::RsaPrivateKey::new
    // already trusts elsewhere in this workspace for key material.
    let password: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(24)
        .map(char::from)
        .collect();
    let password_hash = sms_auth::login::hash_password(&password)
        .map_err(|error| anyhow::anyhow!("hashing the generated password: {error}"))?;

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let ctx = Principal {
        sub: "sms-gateway:provision-user:owner".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    let sys = system_context();

    let user = db
        .user()
        .create(CreateUserInput {
            // The OP is itself the identity source for a locally
            // authenticated user (#194's own login.rs module doc — no
            // external IdP is wired up), so `subject` is simply this row's
            // own id, the same way `authenticate_user`'s Identity
            // construction (app/sms-gateway/src/login.rs) uses `User.id`
            // as `external_id`. `db.user().create` doesn't know its own
            // generated id ahead of the call, so this writes a unique
            // placeholder (subject is @unique — a fixed literal here would
            // make a second concurrent run collide on it before either
            // gets to the corrective update below) and immediately
            // corrects it in a second update — an accepted two-write cost
            // for a one-shot bootstrap command, not a hot path.
            subject: format!("pending-{}", cratestack::uuid::Uuid::new_v4()),
            email: email.clone(),
            displayName: display_name,
            roleKey: role_key,
            lastLoginAt: None,
            deletedAt: None,
        })
        .run(&ctx)
        .await
        .context("creating the User row — check that --role-key names an existing Role")?;

    db.user()
        .update(user.id.clone())
        .set(sms_api::schema::UpdateUserInput {
            subject: Some(user.id.clone()),
            ..Default::default()
        })
        // #59 (landed after this branch): User is @version'd now, and
        // cratestack refuses a versioned update with no If-Match at
        // runtime — cargo check stays green either way.
        .if_match(user.version)
        .run(&ctx)
        .await
        .context("stamping the User row's own id as its subject")?;

    db.user_credential()
        .create(CreateUserCredentialInput {
            userId: user.id.clone(),
            passwordHash: password_hash,
        })
        .run(&sys)
        .await
        .context("creating the UserCredential row")?;

    println!("provisioned user: {email} (id={})", user.id);
    println!("one-time password (never stored, never shown again): {password}");
    println!();
    println!("no password-rotation flow exists yet (#58 tracks the users-and-roles screens) —");
    println!(
        "share this over a channel the recipient controls, not this command's own stdout log."
    );
    Ok(())
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
        use tokio::signal::unix::{signal, SignalKind};
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
