//! The SMS gateway API server.

mod commands;
mod dlr;
mod health;
mod login;
mod op;
mod token_rate_limit;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    Serve(commands::serve::ServeArgs),
    /// Print the generated route table and exit. Needs no database.
    Routes,
    /// Generate a new RSA signing key, activate it, and keep the previous
    /// one publishing in JWKS for `sms_auth::op::ROTATION_OVERLAP` — an
    /// operator action, not a generated-CRUD route (`OauthSigningKey`'s own
    /// schema comment: this is the key that signs every token the OP
    /// issues, and it must never be reachable except as `hasRole('system')`
    /// already restricts it to).
    RotateSigningKey(commands::rotate_signing_key::RotateSigningKeyArgs),
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
    /// `backends/apps/sms-gateway/tests/m1_acceptance_gate_live_postgres.rs` already
    /// does for its own acceptance gate. See #137.
    ///
    /// `ProvisionClientResult` returns `privateKeyPem` exactly once and it
    /// is never stored anywhere in this system (#23/#111) — this command
    /// writes it straight to `--key-out` with `0600` permissions and
    /// refuses to overwrite an existing file, and it is never logged or
    /// printed alongside anything else.
    ProvisionClient(commands::provision_client::ProvisionClientArgs),
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
    /// `ensure_catch_all_route`'s (`commands::seed_dispatch`) own doc for
    /// the small TOCTOU window that shape accepts and why it's fine for an
    /// idempotent ops command. Either way, a Helm `pre-install`/
    /// `pre-upgrade` hook can run this on every install and upgrade
    /// without erroring or duplicating on the second run.
    SeedDispatch(commands::seed_dispatch::SeedDispatchArgs),
    /// Creates the first (or a subsequent) `App` row for a production
    /// deployment — closing the gap `deployment.adoc`'s own "Known seams"
    /// and "Backend-only deployment" sections named explicitly: `App`'s
    /// own `@allow` in `schema.cstack` is `hasRole('owner') ||
    /// hasRole('admin')` on create, nothing this deployment can mint over
    /// HTTP ever carries either role, and `seed-demo-app`/`vsms-demo-seed`
    /// is explicitly demo-only (see that binary's own `--help` text) —
    /// not a substitute for a real production `App`.
    ///
    /// Field choices beyond `--slug`/`--name` reuse
    /// `vsms-demo-seed::create_or_find_demo_app`'s own placeholders
    /// (`monthlyQuota: 1000`, an unrestricted `ipAllowlist`, no GSM-7
    /// transliteration) rather than exposing a flag for every column —
    /// the same "make this deployment able to send something, not a
    /// policy authoring tool" scope `seed-dispatch`'s own catch-all
    /// `Route` already accepts. Adjust the row afterward (the admin
    /// console's own App screen, once one exists, or a direct
    /// generated-CRUD write under a bootstrapped `owner` session) once
    /// the real quota/allowlist for this app is known — a runbook can't
    /// make that business decision for an operator.
    ///
    /// Idempotent: `create` + catching the `23505` on `App.slug`'s
    /// `@unique` index, the same shape every other seed/provision command
    /// in this file uses.
    CreateApp(commands::create_app::CreateAppArgs),
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
    SeedConsoleClient(commands::seed_console_client::SeedConsoleClientArgs),
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
    /// `sms_core::password::hash_password` (Argon2id — never a weaker
    /// scheme just because this is a CLI tool; #52/#58 moved this function
    /// out of `sms-auth::login`, where it lived when this comment was
    /// first written, down to `sms-core` so the console's own
    /// `provisionUser` procedure could call the identical logic), and
    /// prints the plaintext
    /// exactly once — the same "returned once, never stored, never
    /// logged" discipline `ProvisionClient`'s own `privateKeyPem` already
    /// follows, applied here because there is no operator-supplied
    /// `--password` flag: a CLI argument would land in shell history and
    /// the process list, exactly the exposure `write_private_key_pem`'s
    /// own `--key-out` file exists to avoid for the client-provisioning
    /// case.
    ProvisionUser(commands::provision_user::ProvisionUserArgs),
    /// Chains the idempotent steps a fresh deployment needs before
    /// `sms-gateway serve` can ever bind its listener, in the order
    /// `docs/runbooks/deployment.adoc`'s step 3 documents both as one
    /// combined call and, in its own "What `bootstrap` does" subsection,
    /// one sub-step at a time: an OP signing key, the `orange_cm` `Provider` + catch-all
    /// `Route`, the `sms-console` `OauthClient`, and the first operator
    /// account. Deliberately does **not** include `create-app` — a real
    /// production `App`'s quota/allowlist is a business decision this
    /// command can't make for an operator (see that variant's own doc);
    /// this only closes the gap that stops `sms-gateway`/`admin` from
    /// ever starting at all.
    ///
    /// R4 (`CONTRIBUTING.md`: "the admin console is optional, the
    /// backend must run without it") — found in review, not by
    /// inspection: the first cut required `--console-redirect-uri`/
    /// `--owner-email`/`--owner-display-name` unconditionally, which
    /// made this command unusable for a genuinely backend-only
    /// deployment (no console, no operator account, ever). All three
    /// are optional now: omitting `--console-redirect-uri` skips both
    /// the console-client step and the owner-account step outright
    /// (there is no `OauthClient` for a human to log into without a
    /// real `redirect_uri`, and no reason to provision an owner with
    /// nothing to sign into) — see `bootstrap_command`'s own doc for
    /// the exact validation and skip logic.
    ///
    /// Every step reuses the exact function the equivalent standalone
    /// subcommand calls — this is a thinner wrapper chaining them over
    /// one shared connection pool, not a second copy of any of their
    /// logic. Safe to run again against an already-bootstrapped
    /// deployment: signing-key rotation is skipped outright (not just
    /// idempotent — an unconditional rotation here would silently
    /// invalidate every token signed under the previous key's overlap
    /// window sooner than an operator asked for) whenever an `active`
    /// `OauthSigningKey` already exists, `seed-dispatch`'s own two halves
    /// are already idempotent, `seed-console-client`'s is a `23505`
    /// catch, and `provision-user`'s duplicate-email case is reported and
    /// skipped rather than failing the whole chain.
    Bootstrap(commands::bootstrap::BootstrapArgs),
    /// Records the result of a monthly handset check — #64's own "the
    /// structure that records validations," the CLI half of
    /// `docs/runbooks/grey-route-validation.adoc`. Writes a `RouteValidation`
    /// row exactly once per invocation; there is no update path (see
    /// `schema.cstack`'s own comment on `RouteValidation` for why it's
    /// append-only), so a mistaken entry needs a fresh, corrected run of
    /// this command, not a follow-up edit.
    ///
    /// Not a `Procedures` call — `RouteValidation.create`'s own `@@allow`
    /// already admits `hasRole('owner') || hasRole('admin') ||
    /// hasRole('operator')`, so this writes through the generic delegate
    /// under a stand-in `operator`-role context, the same "a CLI acting on
    /// behalf of a human role" shape `ProvisionUser`'s own `owner`-context
    /// write above already uses — not a real token, and never handed back
    /// to a caller.
    RecordRouteValidation(commands::record_route_validation::RecordRouteValidationArgs),
    /// Exec-form liveness/readiness check for orchestrators that can't run
    /// a shell — a distroless `static` runtime image (see
    /// `backends/apps/sms-gateway/Dockerfile`) has no `/bin/sh` and no `curl`, so
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
    Healthcheck(commands::healthcheck::HealthcheckArgs),
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
        Command::Routes => commands::routes::run(),

        Command::Serve(args) => commands::serve::serve_command(args).await,

        Command::RotateSigningKey(args) => {
            commands::rotate_signing_key::rotate_signing_key_command(args.database_url).await
        }

        Command::ProvisionClient(args) => {
            commands::provision_client::provision_client_command(args).await
        }

        Command::SeedDispatch(args) => commands::seed_dispatch::seed_dispatch_command(args).await,

        Command::CreateApp(args) => commands::create_app::create_app_command(args).await,

        Command::SeedConsoleClient(args) => {
            commands::seed_console_client::seed_console_client_command(args).await
        }

        Command::ProvisionUser(args) => {
            commands::provision_user::provision_user_command(args).await
        }

        Command::Bootstrap(args) => commands::bootstrap::bootstrap_command(args).await,

        Command::RecordRouteValidation(args) => {
            commands::record_route_validation::record_route_validation_command(args).await
        }

        Command::Healthcheck(args) => {
            commands::healthcheck::healthcheck_command(&args.addr, &args.path)
        }
    }
}
