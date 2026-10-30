//! The SMS gateway API server.

mod auth_discovery;
mod commands;
mod console_authorize;
mod dlr;
mod grant_gate;
mod health;
mod login;
mod op;
mod op_bearer;
mod sso;
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
    ///
    /// Reads the YAML deployment configuration (`--config`, `--profile`):
    /// the pepper, the pool size, the rate limits and every provider live
    /// there, not in flags (VSC-1; `docs/architecture.md`, section 9.4).
    Serve(commands::serve::ServeArgs),
    /// Print the generated route table and exit. Needs no database.
    Routes,
    /// Generate a new RSA signing key, activate it, and keep the previous one
    /// publishing in JWKS for `op.signing_key.rotation_overlap_secs` — an
    /// operator action, not a generated-CRUD route (`OauthSigningKey`'s own
    /// schema comment: this is the key that signs every token the OP issues, and
    /// it must never be reachable except as `hasRole('system')` already
    /// restricts it to). Run it once on a fresh deployment before `serve`: a
    /// gateway that serves `machine_tokens` refuses to start without an active
    /// key.
    ///
    /// Reads the YAML configuration only to learn whether the deployment mounts
    /// the provider, for `machine_tokens` or for `human_login`: the one key signs
    /// a machine's access token and a person's `id_token`. When it does not
    /// (`op:` is not written, or `auth.methods` permits neither method) it
    /// refuses, `auth_method_disabled`, before touching the database: there is no
    /// provider to sign for, and a key minted anyway would look like a rotation.
    RotateSigningKey(commands::rotate_signing_key::RotateSigningKeyArgs),
    /// Mint a `private_key_jwt` machine client: an `AppClient`, an `OauthClient`
    /// holding only the public JWK, and a fresh RSA keypair whose private half is
    /// written to `--key-out` — an operator action, not a generated-CRUD route,
    /// for the identical reason `RotateSigningKey` above is one: a fresh
    /// deployment has no console key paired to an `owner` or `admin`, so nothing
    /// an operator has at bootstrap could ask for one over HTTP. See #137, and
    /// `sms_auth::provision` for what it writes.
    ///
    /// The private key is returned once and stored nowhere in this system: this
    /// command writes it straight to `--key-out` with `0600` permissions,
    /// refuses to overwrite an existing file, and never logs or prints it.
    ///
    /// A client made here gets a **token** from `/token`; it is not a signing
    /// key, and the API accepts the token as a bearer (VSA-3b). It refuses on a
    /// deployment that serves no tokens, `auth_method_disabled`, and points at
    /// `register-client`, which a deployment that permits `keypair` serves.
    ProvisionClient(commands::provision_client::ProvisionClientArgs),
    /// Registers an Ed25519 public key for an `App`'s signing client (VS-2),
    /// the only way a machine caller gets in. The caller generates the keypair
    /// and hands over only the public half; vsms never sees a private key, so
    /// nothing is returned once and nothing can be lost. Creates the app's
    /// signing client (`appc_<slug>`) the first time, then attaches the key.
    ///
    /// Idempotent on the key's RFC 9679 thumbprint: running it twice with the
    /// same key leaves one `ClientKey` and exits 0. A key that was revoked, or
    /// that belongs to another client, is refused rather than reinstated or
    /// moved. It goes through the real `registerAppClientKey` entry point, so
    /// `@allow` is enforced here exactly as it is over HTTP (an operator
    /// action rather than a generated-CRUD route: a fresh deployment has no
    /// console key paired to an `owner`/`admin` yet, so nothing could call it
    /// over HTTP).
    RegisterClient(commands::register_client::RegisterClientArgs),
    /// Break-glass: registers a console public key (P-256, from a browser's
    /// `WebCrypto`) for an **existing, active** user, so that person can sign
    /// in to the console without a phone pairing (signed transport, VS-5a).
    /// Run it inside the cluster; it is the operator action behind "owner
    /// ruling Q2: break-glass is CLI key enrolment", and the only way in when
    /// no vaam admin phone is available.
    ///
    /// No proof of possession is asked for: whoever can run this against the
    /// database already holds more authority than a pairing grants, the same
    /// reasoning `provision-user` rests on. For the same reason it never
    /// creates a user and never changes a role - it cannot mint an owner,
    /// and `--user` must name someone who already exists (a locked-out
    /// owner included). The key lasts 30 days, as a paired one does.
    ///
    /// Idempotent on the key's RFC 9679 thumbprint: running it twice with the
    /// same key and user leaves one `ConsoleKey` and exits 0. A key that is
    /// revoked, expired or another user's is refused, never reinstated or
    /// moved.
    PairConsoleKey(commands::pair_console_key::PairConsoleKeyArgs),
    /// Seed (or reactivate) the `Provider` row `resolve_provider_row_id`
    /// (this file) resolves at startup, **and** a catch-all `Route`
    /// pointing at it — an operator action, not a generated-CRUD route,
    /// for the same reason `RegisterClient` above is one: `Provider`'s own `@allow` in `schema.cstack` admits only
    /// `hasRole('owner') || hasRole('admin')` on create (`Route`'s is
    /// identical), and the only way to be either over HTTP is a console key
    /// paired to a person (VS-3), which a fresh deployment has none of.
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
    /// Creates a `User` row - the account a person's console key is later
    /// paired to. An operator action, not a generated-CRUD route: `User`'s own
    /// `@allow` in `schema.cstack` admits `hasRole('owner') || hasRole('admin')`
    /// on create, and a fresh deployment has no console key paired to either
    /// yet, so the very first account is necessarily made this way.
    ///
    /// Makes **no credential** by default: the person signs in by pairing a
    /// browser key, through a vaam admin phone or with `pair-console-key` (the
    /// break-glass), which attaches to the user this command made. Idempotent on
    /// the email.
    ///
    /// With `--password --password-out <path>` (VSA-4a), on a deployment that
    /// serves `human_login`, it also gives the person a generated password,
    /// written to the path (`0600`, `O_EXCL`, never overwriting) and **never
    /// printed**. An existing account without a password gets one; an existing
    /// password is refused unless `--reset`. On a deployment that does not serve
    /// `human_login` it refuses, `auth_method_disabled`, before the database is
    /// touched. The password session's token is accepted by the API (VSA-4b) as the
    /// person, except that an `owner` or an `admin` behind it is a 403: those
    /// roles act only from a paired console key (`pair-console-key`).
    ProvisionUser(commands::provision_user::ProvisionUserArgs),
    /// Registers (or reconciles) the console's `OauthClient` row from
    /// `human_login.console_client`: the public client a person's sign-in is
    /// issued to, with the `authorization_code` grant alone, the `openid profile`
    /// scopes, PKCE required and exactly the configured redirect URIs. An
    /// operator action: `OauthClient` is `hasRole('system')` only and has no route.
    ///
    /// **Reconciles** rather than only creating: a row that differs from the
    /// configuration (a changed redirect URI, an `authorization_code
    /// refresh_token` grant left from before #446) is brought into line, and an
    /// operator is told what was wrong. `serve` refuses to start on a row that
    /// does not match, naming this command. Idempotent. Refuses,
    /// `auth_method_disabled`, before the database is touched when the deployment
    /// does not serve `human_login`.
    SeedConsoleClient(commands::seed_console_client::SeedConsoleClientArgs),
    /// Chains the idempotent steps a fresh deployment needs before
    /// `sms-gateway serve` can do anything useful, in the order
    /// `docs/runbooks/deployment.adoc`'s step 3 documents both as one
    /// combined call and, in its own "What `bootstrap` does" subsection,
    /// one sub-step at a time: the `orange_cm` `Provider` + catch-all
    /// `Route`, and the first operator account. Deliberately does **not**
    /// include `create-app` — a real production `App`'s quota/allowlist is a
    /// business decision this command can't make for an operator (see that
    /// variant's own doc).
    ///
    /// R4 (`CONTRIBUTING.md`: "the admin console is optional, the
    /// backend must run without it"): `--owner-email` and
    /// `--owner-display-name` are optional, both or neither. Omitting them
    /// skips the owner-account step, for a genuinely backend-only
    /// deployment (no console, no operator account, ever).
    ///
    /// Every step reuses the exact function the equivalent standalone
    /// subcommand calls — this is a thinner wrapper chaining them over
    /// one shared connection pool, not a second copy of any of their
    /// logic. Safe to run again against an already-bootstrapped
    /// deployment: `seed-dispatch`'s own two halves are already
    /// idempotent, and `provision-user`'s duplicate-email case is reported
    /// and skipped rather than failing the whole chain.
    ///
    /// `--owner-password-out <path>` (VSA-4a) also gives the first owner a
    /// generated password, written to the path (`0600`, `O_EXCL`, never
    /// printed), and registers the console client (`seed-console-client`) in the
    /// same run. Needs `--owner-email` and a deployment that serves
    /// `human_login`: otherwise it refuses, `auth_method_disabled`, before the
    /// database is touched. Without the flag `bootstrap` reads no configuration.
    ///
    /// `--skip-seed-dispatch` skips step 1 (the hardcoded `orange_cm`
    /// `Provider` + catch-all `Route`, priority 0, weight 1). Use it when
    /// the deployment will not send through Orange — an AWS-only go-live,
    /// say — and run `seed-dispatch` yourself for the provider you will
    /// actually use. Without it, a later `seed-dispatch --key aws_sms ...`
    /// adds a *second* identical catch-all, the routing engine draws 50/50
    /// between them, and half of all traffic is routed to `orange_cm`,
    /// which the worker has no credentials for (dispatch retries those
    /// messages forever). Default off, so existing invocations are
    /// unchanged.
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
    /// Registers this deployment's `POST /dlr/mtn_cm` endpoint with MADAPI
    /// so MTN will actually send delivery receipts to it.
    ///
    /// This is the step that has no Orange equivalent. Orange's DR
    /// endpoint is whitelisted by a manual support ticket
    /// (`docs/runbooks/36-handset-gate.adoc`); MTN's is a real API call
    /// (`POST /messages/sms/subscription`, vendored Swagger's
    /// `ShortCodeSubscription`), so it can be automated — which is why
    /// this exists as a subcommand rather than a runbook paragraph.
    ///
    /// Run once per approved `serviceCode`, before expecting any DLR.
    /// Sending alone is not enough: MADAPI requires
    /// `requestDeliveryReceipt: true` on each outbound message *and* a
    /// registered `deliveryReportUrl`. The adapter always sends the
    /// former; this command supplies the latter. Miss it and messages
    /// submit successfully, reach `submitted`, and then sit there with no
    /// receipt ever arriving — which looks exactly like a broken route.
    ///
    /// Needs no database: it talks only to MADAPI, which is why it is the
    /// one subcommand here with no `--database-url`. MTN's credentials are
    /// the `providers[code=mtn_cm]` entry of the YAML configuration.
    MtnSubscribeDlr(commands::mtn_subscribe_dlr::MtnSubscribeDlrArgs),

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

    /// Print the two public keys an integrator pins to verify answers
    /// (`current` and `next`), as unpadded base64url, one per line. The
    /// private halves are `serve`'s `--envelope-signing-key-file` and
    /// `--envelope-next-signing-key-file`; nothing secret is printed. Needs
    /// no database.
    ServerKeys(commands::server_keys::ServerKeysArgs),
}

/// Installs `ring` as the process-wide default `rustls` `CryptoProvider`.
///
/// Names the provider once, before any HTTP client exists, so no TLS client
/// in this process depends on which one happens to be built first. Every
/// client here (`reqwest` with `rustls-tls` in the provider adapters,
/// cratestack's own) is built on `ring`, and `aws-lc-rs` is not in the
/// dependency graph at all (`cargo tree -i aws-lc-rs` finds nothing) — its
/// `cmake`/pkg-config build requirement is what made a musl build
/// impractical. The embedded `OpenID` Provider's `authkestra` stack, whose
/// `rustls-no-provider` clients genuinely panicked without this call, was
/// deleted in VS-4; the call stays because it is cheap and explicit.
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
                // `sms_provider_aws_sms`: the SNS
                // subscription-confirmation command (with its token) and the
                // SignatureVersion-1 / unsubscribe errors are logged there.
                .unwrap_or_else(|_| {
                    "sms_gateway=info,sms_api=info,sms_auth=info,sms_provider_aws_sms=info,cratestack=info".into()
                }),
        )
        .init();

    // VSC-1: before clap parses, so a flag that moved into the YAML is refused
    // naming the key that replaced it, not with "unexpected argument".
    sms_config::reject_retired_flags(std::env::args_os().skip(1))?;

    match Cli::parse().command {
        Command::Routes => commands::routes::run(),

        Command::RotateSigningKey(args) => {
            commands::rotate_signing_key::rotate_signing_key_command(args).await
        }

        Command::ProvisionClient(args) => {
            commands::provision_client::provision_client_command(args).await
        }

        Command::Serve(args) => commands::serve::serve_command(args).await,

        Command::RegisterClient(args) => {
            commands::register_client::register_client_command(args).await
        }

        Command::PairConsoleKey(args) => {
            commands::pair_console_key::pair_console_key_command(args).await
        }

        Command::SeedDispatch(args) => commands::seed_dispatch::seed_dispatch_command(args).await,

        Command::CreateApp(args) => commands::create_app::create_app_command(args).await,

        Command::ProvisionUser(args) => {
            commands::provision_user::provision_user_command(args).await
        }

        Command::SeedConsoleClient(args) => {
            commands::seed_console_client::seed_console_client_command(args).await
        }

        Command::Bootstrap(args) => commands::bootstrap::bootstrap_command(args).await,

        Command::RecordRouteValidation(args) => {
            commands::record_route_validation::record_route_validation_command(args).await
        }

        Command::MtnSubscribeDlr(args) => {
            commands::mtn_subscribe_dlr::mtn_subscribe_dlr_command(args).await
        }

        Command::Healthcheck(args) => {
            commands::healthcheck::healthcheck_command(&args.addr, &args.path)
        }

        Command::ServerKeys(args) => commands::server_keys::server_keys_command(&args),
    }
}

#[cfg(test)]
mod tests {
    use clap::{Command, CommandFactory};

    /// Every argument of `command` and of every subcommand beneath it.
    fn arguments(command: &Command) -> Vec<&clap::Arg> {
        let mut found: Vec<&clap::Arg> = command.get_arguments().collect();
        for sub in command.get_subcommands() {
            found.extend(arguments(sub));
        }
        found
    }

    /// The flags VSC-1 moved into the YAML are not flags any more, and none is
    /// bound to its old variable: clap would otherwise read the variable the
    /// retired-variable check exists to refuse. Re-adding any one of them to
    /// any subcommand fails here.
    #[test]
    fn no_subcommand_still_declares_a_retired_flag_or_variable() {
        let command = super::Cli::command();
        for arg in arguments(&command) {
            if let Some(long) = arg.get_long() {
                assert!(
                    !sms_config::RETIRED
                        .iter()
                        .any(|retired| retired.flag == format!("--{long}")),
                    "--{long} is retired (sms_config::RETIRED) and is declared again"
                );
            }
            if let Some(env) = arg.get_env().and_then(|env| env.to_str()) {
                assert!(
                    !sms_config::RETIRED.iter().any(|retired| retired.env == env),
                    "{env} is retired (sms_config::RETIRED) and is bound to an argument again"
                );
            }
        }
    }

    /// The commands that read the YAML take `--config` and `--profile` (bound to
    /// `SMS_CONFIG` and `SMS_PROFILE`); the ones that read nothing do not grow
    /// them. `seed-dispatch` has an unrelated `--config` (the provider's JSON
    /// settings), which is why this looks at the variable and not the name.
    /// `provision-client` and `rotate-signing-key` read it since VSA-3a, to learn
    /// whether the deployment serves `machine_tokens`; `register-client` and
    /// `pair-console-key` since VSA-3b, to learn whether it permits `keypair`.
    #[test]
    fn exactly_the_commands_that_need_the_configuration_take_its_options() {
        let command = super::Cli::command();
        let mut with_config: Vec<&str> = command
            .get_subcommands()
            .filter(|sub| {
                sub.get_arguments()
                    .any(|arg| arg.get_env().is_some_and(|env| env == "SMS_CONFIG"))
            })
            .map(Command::get_name)
            .collect();
        with_config.sort_unstable();
        assert_eq!(
            with_config,
            [
                "bootstrap",
                "mtn-subscribe-dlr",
                "pair-console-key",
                "provision-client",
                "provision-user",
                "register-client",
                "rotate-signing-key",
                "seed-console-client",
                "serve"
            ]
        );
    }

    /// A build without the `fake-aws` feature - every release image - has no
    /// flag that names a fake certificate authority at all.
    #[cfg(not(feature = "fake-aws"))]
    #[test]
    fn a_release_build_has_no_fake_certificate_flag() {
        use clap::Parser;

        let error = super::Cli::try_parse_from([
            "sms-gateway",
            "serve",
            "--aws-sms-fake-sns-cert-base",
            "http://sms-fake-aws:8095/",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
