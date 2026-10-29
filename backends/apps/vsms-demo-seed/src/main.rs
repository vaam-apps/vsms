#![doc = include_str!("main.md")]

use anyhow::{Context, Result, bail};
use clap::Parser;
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateAppInput, CreateSenderIdInput, CreateSenderIdRegistrationInput,
    CreateWebhookEndpointInput, SenderIdKind, SenderIdRegistrationStatus, UpdateSenderIdInput,
    WebhookDeliveryMode, app as app_filter, provider as provider_filter,
    sender_id as sender_id_filter, sender_id_registration as sender_id_registration_filter,
    webhook_endpoint as webhook_endpoint_filter,
};
use sms_api::{Principal, PrincipalKind};

/// The `message.*` event catalogue §8.4/`sms_api::webhooks::message_event_type`
/// actually emits — the full set, since this is a demo endpoint meant to
/// show every transition an evaluator's one message can plausibly reach,
/// not a narrowly-scoped production integration. Packed with
/// `sms_core::pack` (§2.0's sentinel-wrapped-string convention —
/// `WebhookEndpoint.eventTypes` matching, `backends/crates/sms-api/src/webhooks.rs`,
/// is `.contains(sms_core::needle(event_type))` against exactly this
/// shape).
const DEMO_WEBHOOK_EVENT_TYPES: &[&str] = &[
    "message.accepted",
    "message.submitted",
    "message.delivered",
    "message.failed",
    "message.expired",
    "message.uncertain",
    "message.cancelled",
];

/// Command-line surface — same flags, same defaults, as the
/// `sms-gateway seed-demo-app` subcommand this replaces.
#[derive(Debug, Parser)]
#[command(
    name = "vsms-demo-seed",
    version,
    about = "Demo-only: seeds the App/SenderId fixtures compose.demo.yaml needs. Never point this at a production database."
)]
struct Cli {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    database_url: String,

    #[arg(long, default_value = "vsms demo app")]
    name: String,

    /// `App.slug`'s own `@regex` — lowercase alphanumeric and `-` only.
    #[arg(long, default_value = "vsms-demo")]
    slug: String,

    /// The `SenderId.value` this demo sends under — not a real,
    /// Orange-approved sender name (see this crate's own module doc): the
    /// registration this command writes is approved in this database
    /// only, never checked against any real provider.
    #[arg(long, default_value = "VSMS")]
    sender_id: String,

    /// Must already exist — the `Provider.key` `sms-gateway seed-dispatch`
    /// seeded.
    #[arg(long, default_value = "orange_cm")]
    provider_key: String,

    /// Which of `App`/`SenderId`'s create-admitted roles to run this call
    /// under.
    #[arg(long, default_value = "owner")]
    role: String,

    /// The demo app's own inbound receiver — `examples/node/demo-app`,
    /// mounted at this address by the `demo` (never `console`- or
    /// backend-only-) profiled `demo-app` service in both
    /// `compose.dev.yaml` and `compose.demo.yaml`. Deliberately no
    /// default and no `WebhookEndpoint` at all when this flag is
    /// omitted — R4 (CONTRIBUTING.md: "the backend must run without the
    /// console") means a *backend-only* `up -d`, with no `demo-app`
    /// container ever started, must not seed an endpoint pointed at a
    /// host that will never resolve: `hooks` would mark every attempt
    /// `dead`, open the circuit, and log an error every tick, forever,
    /// on a deployment that never asked for any of it. Passed only by
    /// the separate, `--profile demo`-gated `seed-demo-webhook` service
    /// (`compose.dev.yaml`/`compose.demo.yaml`), never by the
    /// unconditional `seed-demo-app` step. `WebhookEndpoint.url`
    /// (`@uri`) has no reachability check at write time regardless —
    /// this only has to resolve once `hooks` actually tries to deliver
    /// to it, well after this command exits.
    #[arg(long)]
    webhook_url: Option<String>,

    /// #59/#40: `hooks.rs`'s own endpoint-bookkeeping precedent — a
    /// generous-but-bounded retry budget for a demo endpoint that should
    /// come up quickly (the worker's `hooks` role starts around the same
    /// time as `demo-app` itself), not §6.3-style production tuning.
    #[arg(long, default_value_t = 8)]
    webhook_max_attempts: i64,

    /// §4.4/`webhooks.rs::message_payload`: masking `to` behind
    /// `Msisdn::masked` is the right default for a real integration, but
    /// this demo exists to show an evaluator the whole round trip
    /// end to end — an unmasked MSISDN in the printed timeline is more
    /// legible than a partially-redacted one for a stack nobody else can
    /// see.
    #[arg(long, default_value_t = false)]
    webhook_mask_recipient: bool,

    /// VS-8: the audience a `sealed` delivery to `--webhook-url` binds — the
    /// name the receiver's `EnvelopeLayer` is built with (`demo-app` opens
    /// deliveries as `DEMO_WEBHOOK_AUDIENCE`, defaulting to this same value).
    /// Passing it makes the endpoint `sealed`, the recommended mode and the
    /// one the compose stacks seed: there is no secret to hand the receiver,
    /// which pins the gateway's public keys (`sms-gateway server-keys`) and
    /// checks the audience. Exactly one of this and `--webhook-secret-out` is
    /// required alongside `--webhook-url`, and either is refused without it, so
    /// a stray flag never looks like it did something; no default, because a
    /// default would be a receiver identity nobody chose.
    #[arg(long, conflicts_with = "webhook_secret_out")]
    webhook_audience: Option<String>,

    /// Where to write the signing secret of an `hmac` endpoint (VS-8R) — the
    /// legacy mode, for a receiver that still verifies `X-Sms-Signature`.
    /// Passing it makes the endpoint `hmac`: the secret is freshly generated
    /// (`sms_webhook::generate_secret()`) on first run, or the existing row's
    /// own `secret` on every rerun against an already-seeded database, and
    /// written only here, never printed (stdout is a container log, which is
    /// where a signing secret must not end up). `demo-app` has no other way to
    /// learn it. Ordinary create-or-truncate semantics, no restrictive mode: this
    /// whole stack is a disposable, obviously-demo deployment.
    #[arg(long, conflicts_with = "webhook_audience")]
    webhook_secret_out: Option<std::path::PathBuf>,

    /// Where to write the id of the `WebhookEndpoint` this call created or
    /// found. A receiver serves exactly one endpoint and its opener is built
    /// with that endpoint's id, which the gateway mints, so the demo's receiver
    /// (`demo-app`) cannot be configured until the row exists: the compose
    /// stacks share the file through a volume. Refused without `--webhook-url`,
    /// so a stray flag never looks like it did something.
    #[arg(long)]
    webhook_endpoint_id_out: Option<std::path::PathBuf>,
}

/// `create` the `App` row, or resolve the id of the one that already
/// exists. Same create-then-catch-`23505` shape `sms-gateway
/// seed-dispatch`'s own `create_or_find_provider` uses: `App.slug` is
/// `@unique`.
async fn create_or_find_demo_app(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    name: &str,
    slug: &str,
) -> Result<String> {
    match db
        .app()
        .create(CreateAppInput {
            name: name.to_owned(),
            slug: slug.to_owned(),
            description: Some("seeded by vsms-demo-seed for compose.demo.yaml".to_owned()),
            defaultSenderIdId: None,
            monthlyQuota: 1000,
            // A packed-string field with sentinel separators (§2.0) — the
            // empty-list encoding, matching `send_test_message.rs`'s own
            // literal `" "`. An unfiltered demo has no IP allowlist to
            // enforce.
            ipAllowlist: " ".to_owned(),
            transliterateToGsm7: false,
            deletedAt: None,
        })
        .run(ctx)
        .await
    {
        Ok(created) => {
            println!("created App {} (slug={slug:?})", created.id);
            Ok(created.id)
        }
        Err(e) if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION) => {
            let existing = db
                .app()
                .find_many()
                .where_expr(FilterExpr::from(app_filter::slug().eq(slug.to_owned())))
                .limit(1)
                .run(ctx)
                .await
                .context("looking up the existing App row after a duplicate-slug create")?;
            let row = existing.into_iter().next().with_context(|| {
                format!("App row with slug {slug:?} reported as a duplicate on create but not found on lookup")
            })?;
            println!("App {} (slug={slug:?}) already exists", row.id);
            Ok(row.id)
        }
        Err(e) => Err(e).context("creating the App row"),
    }
}

/// Ensure an `active` `SenderId` with an `approved` `SenderIdRegistration`
/// against `provider_id` exists. `Procedures::resolve_sender_id`
/// (`backends/crates/sms-api/src/procedures.rs`) is the real reader this satisfies:
/// it requires both, not just one or the other, matching
/// `send_test_message.rs`'s own `ensure_sender_ready`.
async fn ensure_demo_sender_id(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    value: &str,
    provider_id: &str,
) -> Result<()> {
    let existing = db
        .sender_id()
        .find_many()
        .where_expr(FilterExpr::from(
            sender_id_filter::value().eq(value.to_owned()),
        ))
        .limit(1)
        .run(ctx)
        .await
        .context("looking up an existing SenderId")?;

    let (sender_id_row_id, sender_id_version, already_active) =
        if let Some(row) = existing.into_iter().next() {
            println!("reusing existing SenderId {value:?} ({})", row.id);
            (row.id, row.version, row.active)
        } else {
            let created = db
                .sender_id()
                .create(CreateSenderIdInput {
                    value: value.to_owned(),
                    kind: if value.chars().all(|c| c.is_ascii_digit()) {
                        SenderIdKind::shortcode
                    } else {
                        SenderIdKind::alphanumeric
                    },
                    notes: Some(
                        "seeded by vsms-demo-seed for compose.demo.yaml — not a real, \
                         provider-approved sender"
                            .to_owned(),
                    ),
                })
                .run(ctx)
                .await
                .context("creating the SenderId row")?;
            println!("created SenderId {value:?} ({})", created.id);
            (created.id, created.version, false)
        };

    let has_approved_registration = !db
        .sender_id_registration()
        .find_many()
        .where_expr(
            FilterExpr::from(
                sender_id_registration_filter::senderIdId().eq(sender_id_row_id.clone()),
            )
            .and(sender_id_registration_filter::status().eq("approved".to_owned())),
        )
        .limit(1)
        .run(ctx)
        .await
        .context("checking for an existing approved SenderIdRegistration")?
        .is_empty();

    if has_approved_registration {
        println!("SenderId {value:?} already has an approved registration");
    } else {
        db.sender_id_registration()
            .create(CreateSenderIdRegistrationInput {
                senderIdId: sender_id_row_id.clone(),
                providerId: provider_id.to_owned(),
                status: SenderIdRegistrationStatus::approved,
                submittedAt: None,
                approvedAt: None,
                reference: None,
                rejectionReason: None,
            })
            .run(ctx)
            .await
            .context("creating the SenderIdRegistration row")?;
        println!("registered SenderId {value:?} as approved (provider={provider_id})");
    }

    if already_active {
        println!("SenderId {value:?} already active");
    } else {
        db.sender_id()
            .update(sender_id_row_id.clone())
            .set(UpdateSenderIdInput {
                active: Some(true),
                ..Default::default()
            })
            .if_match(sender_id_version)
            .run(ctx)
            .await
            .context("activating the SenderId row")?;
        println!("activated SenderId {value:?}");
    }

    Ok(())
}

/// How the demo endpoint is delivered to, chosen by which of `--webhook-audience`
/// and `--webhook-secret-out` was passed ([`delivery_plan`]).
#[derive(Debug, PartialEq, Eq)]
enum Delivery<'a> {
    /// A sealed delivery to this audience.
    Sealed { audience: &'a str },
    /// An HMAC-signed delivery; the secret goes to the file at this path. Made
    /// through the ORM whatever `auth.methods` says (this binary does not read the
    /// YAML): a demo opt-in whose deliveries a deployment without `hmac_webhooks`
    /// holds back (`main.md`, "`--webhook-secret-out` and `auth.methods`").
    Hmac { secret_path: &'a std::path::Path },
}

impl Delivery<'_> {
    /// The mode's name, the only thing about a delivery that is printed.
    const fn mode(&self) -> &'static str {
        match self {
            Self::Sealed { .. } => "sealed",
            Self::Hmac { .. } => "hmac",
        }
    }
}

/// Writes the webhook secret to `path`, readable by its owner alone. The mode
/// is set at creation (no window where the file is world-readable) and again
/// afterwards, for a file that already existed with another one.
fn write_secret_file(path: &std::path::Path, secret: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(secret.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// What the webhook flags ask for, or why they are wrong. Pure so the rules are
/// tested without a database: exactly one of the two mode flags goes with
/// `--webhook-url`, and neither goes without it.
fn delivery_plan<'a>(
    url: Option<&'a str>,
    audience: Option<&'a str>,
    secret_path: Option<&'a std::path::Path>,
) -> Result<Option<(&'a str, Delivery<'a>)>> {
    match (url, audience, secret_path) {
        (Some(url), Some(audience), None) => Ok(Some((url, Delivery::Sealed { audience }))),
        (Some(url), None, Some(secret_path)) => Ok(Some((url, Delivery::Hmac { secret_path }))),
        (Some(_), Some(_), Some(_)) => bail!(
            "--webhook-audience and --webhook-secret-out were both given — an endpoint is sealed \
             (an audience) or hmac (a secret), not both. Pass one."
        ),
        (Some(_), None, None) => bail!(
            "--webhook-url was given without --webhook-audience or --webhook-secret-out — the \
             endpoint must say how its deliveries are authenticated. Pass --webhook-audience \
             <name> (sealed, recommended: the name the receiver's EnvelopeLayer is built with) \
             or --webhook-secret-out <path> (hmac: where the receiver reads the signing secret)."
        ),
        (None, Some(_), _) => bail!(
            "--webhook-audience was given but --webhook-url was not — nothing would ever use it. \
             Pass --webhook-url too, or drop --webhook-audience."
        ),
        (None, None, Some(_)) => bail!(
            "--webhook-secret-out was given but --webhook-url was not — nothing would ever be \
             written to it. Pass --webhook-url too, or drop --webhook-secret-out."
        ),
        (None, None, None) => Ok(None),
    }
}

/// The endpoint this call created or found, and the secret its receiver needs
/// (`hmac` only).
struct DemoEndpoint {
    id: String,
    secret: Option<String>,
}

/// Ensure a `WebhookEndpoint` pointed at the demo app's own receiver exists.
/// `WebhookEndpoint` carries no unique constraint on `(appId, url)` (unlike
/// `App.slug`), so idempotency here is read-then-create, the same
/// accepted-TOCTOU-window shape `ensure_demo_sender_id`'s own
/// approved-registration check already uses — safe for the same reason: this
/// only ever runs from one container in one sequential compose dependency
/// chain, never concurrently with itself.
///
/// A row that already exists keeps the mode, audience and secret it was created
/// with; a rerun that asks for something else says so instead of silently
/// editing an endpoint an operator may have changed in the console. For an
/// `hmac` endpoint that means the row's own `secret` is the one written back,
/// so `demo-app` and the database never disagree about which value is current,
/// even across a restart that skips re-creation. An existing `sealed` row
/// asked for as `hmac` has no secret to write, and that is an error.
async fn ensure_demo_webhook_endpoint(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    app_id: &str,
    url: &str,
    delivery: &Delivery<'_>,
    max_attempts: i64,
    mask_recipient: bool,
) -> Result<DemoEndpoint> {
    let existing = db
        .webhook_endpoint()
        .find_many()
        .where_expr(
            FilterExpr::from(webhook_endpoint_filter::appId().eq(app_id.to_owned()))
                .and(webhook_endpoint_filter::url().eq(url.to_owned())),
        )
        .limit(1)
        .run(ctx)
        .await
        .context("looking up an existing WebhookEndpoint")?;

    if let Some(row) = existing.into_iter().next() {
        println!("reusing existing WebhookEndpoint {} (url={url:?})", row.id);
        let asked = match delivery {
            Delivery::Sealed { audience } => {
                row.deliveryMode == WebhookDeliveryMode::sealed
                    && row.audience.as_deref() == Some(*audience)
            }
            Delivery::Hmac { .. } => row.deliveryMode == WebhookDeliveryMode::hmac,
        };
        if !asked {
            println!(
                "note: it keeps its delivery mode ({:?}) and audience ({:?}), not what was \
                 passed now",
                row.deliveryMode, row.audience
            );
        }
        if matches!(delivery, Delivery::Hmac { .. }) && row.secret.is_none() {
            bail!(
                "the existing WebhookEndpoint {} is sealed and has no secret to write to \
                 --webhook-secret-out; delete it, or switch it to hmac in the console",
                row.id
            );
        }
        return Ok(DemoEndpoint {
            id: row.id,
            secret: row.secret,
        });
    }

    let event_types =
        sms_core::pack(DEMO_WEBHOOK_EVENT_TYPES.iter().copied()).with_context(|| {
            format!(
                "packing DEMO_WEBHOOK_EVENT_TYPES ({DEMO_WEBHOOK_EVENT_TYPES:?}) — a literal, \
                 space-free array, should never fail to pack"
            )
        })?;
    let (delivery_mode, audience, secret) = match delivery {
        Delivery::Sealed { audience } => (
            WebhookDeliveryMode::sealed,
            Some((*audience).to_owned()),
            None,
        ),
        Delivery::Hmac { .. } => (
            WebhookDeliveryMode::hmac,
            None,
            Some(sms_webhook::generate_secret()),
        ),
    };

    let created = db
        .webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            deliveryMode: delivery_mode,
            audience,
            secret,
            prevSecret: None,
            secretRotatedAt: None,
            eventTypes: event_types,
            maskRecipient: mask_recipient,
            maxAttempts: max_attempts,
            circuitOpenUntil: None,
        })
        .run(ctx)
        .await
        .context("creating the WebhookEndpoint row")?;
    println!(
        "created WebhookEndpoint {} (url={url:?}, mode={:?})",
        created.id, created.deliveryMode
    );

    Ok(DemoEndpoint {
        id: created.id,
        secret: created.secret,
    })
}

/// `--webhook-endpoint-id-out` without `--webhook-url` would look like it did
/// something: no endpoint exists to write the id of.
fn refuse_endpoint_id_out_without_url(
    out: Option<&std::path::Path>,
    url: Option<&str>,
) -> Result<()> {
    if out.is_some() && url.is_none() {
        bail!(
            "--webhook-endpoint-id-out was given but --webhook-url was not — no WebhookEndpoint \
             would exist to write the id of. Pass --webhook-url (and --webhook-audience) too, or \
             drop --webhook-endpoint-id-out."
        );
    }
    Ok(())
}

/// Writes the endpoint's id where `--webhook-endpoint-id-out` says, if it does.
fn write_endpoint_id(out: Option<&std::path::Path>, endpoint_id: &str) -> Result<()> {
    if let Some(path) = out {
        std::fs::write(path, endpoint_id)
            .with_context(|| format!("writing the endpoint id to {}", path.display()))?;
        println!("wrote the endpoint id to {}", path.display());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "vsms_demo_seed=info,sms_api=info,cratestack=info".into()),
        )
        .init();

    let Cli {
        database_url,
        name,
        slug,
        sender_id,
        provider_key,
        role,
        webhook_url,
        webhook_max_attempts,
        webhook_mask_recipient,
        webhook_audience,
        webhook_secret_out,
        webhook_endpoint_id_out,
    } = Cli::parse();

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — App's/SenderId's own @allow admit nothing \
             else on create, got {role:?}"
        );
    }

    // Same conservative pool size sms-gateway's own one-shot seed/provision
    // commands use, and for the same reason: this is a one-shot process
    // writing a handful of @@audit-backed rows (App, SenderId,
    // SenderIdRegistration).
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();

    let ctx = Principal {
        sub: format!("vsms-demo-seed:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let provider_row = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider_filter::key().eq(provider_key.clone()),
        ))
        .limit(1)
        .run(&ctx)
        .await
        .context("looking up the Provider row this SenderIdRegistration references")?
        .into_iter()
        .next()
        .with_context(|| {
            format!(
                "no Provider with key {provider_key:?} exists yet — run `sms-gateway \
                 seed-dispatch` first"
            )
        })?;

    let app_id = create_or_find_demo_app(&db, &ctx, &name, &slug).await?;
    ensure_demo_sender_id(&db, &ctx, &sender_id, &provider_row.id).await?;

    // Conditional on purpose (see `webhook_url`'s own doc comment): no
    // `--webhook-url` means no `WebhookEndpoint` at all, not one pointed
    // at a default that only resolves inside a stack that also runs
    // `demo-app` — R4 (CONTRIBUTING.md): a backend-only deployment must
    // not silently get an endpoint that will error on every delivery
    // attempt forever. `--webhook-audience` with no `--webhook-url` is
    // a caller error, not a silent no-op — it would otherwise look like
    // it did something.
    refuse_endpoint_id_out_without_url(webhook_endpoint_id_out.as_deref(), webhook_url.as_deref())?;
    match delivery_plan(
        webhook_url.as_deref(),
        webhook_audience.as_deref(),
        webhook_secret_out.as_deref(),
    )? {
        Some((url, delivery)) => {
            let endpoint = ensure_demo_webhook_endpoint(
                &db,
                &ctx,
                &app_id,
                url,
                &delivery,
                webhook_max_attempts,
                webhook_mask_recipient,
            )
            .await?;
            write_endpoint_id(webhook_endpoint_id_out.as_deref(), &endpoint.id)?;

            // The secret is never printed: stdout is a container log, and a
            // log is exactly where a signing secret must not end up (CodeQL
            // flagged the pre-sealed fallback as cleartext logging).
            // `--webhook-secret-out` is the only way to receive it.
            if let (Delivery::Hmac { secret_path }, Some(secret)) = (&delivery, &endpoint.secret) {
                write_secret_file(secret_path, secret).with_context(|| {
                    format!("writing the webhook secret to {}", secret_path.display())
                })?;
                println!("webhook secret written to: {}", secret_path.display());
            }

            println!(
                "demo App/SenderId/WebhookEndpoint fixtures ready (slug={slug:?}, sender={sender_id:?}, webhook={url:?}, delivery={})",
                delivery.mode()
            );
        }
        None => {
            println!(
                "demo App/SenderId fixtures ready (slug={slug:?}, sender={sender_id:?}); no \
                 --webhook-url given, so no WebhookEndpoint was seeded"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Delivery, delivery_plan};
    use std::path::Path;

    #[test]
    fn an_audience_makes_the_endpoint_sealed_and_a_secret_file_makes_it_hmac() {
        let sealed = delivery_plan(
            Some("http://demo-app:9000/webhooks"),
            Some("demo-app"),
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            sealed,
            (
                "http://demo-app:9000/webhooks",
                Delivery::Sealed {
                    audience: "demo-app"
                }
            )
        );
        let hmac = delivery_plan(
            Some("http://demo-app:9000/webhooks"),
            None,
            Some(Path::new("/secrets/webhook-secret")),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            hmac,
            (
                "http://demo-app:9000/webhooks",
                Delivery::Hmac {
                    secret_path: Path::new("/secrets/webhook-secret")
                }
            )
        );
    }

    #[test]
    fn a_delivery_prints_as_its_mode_alone() {
        let hmac = Delivery::Hmac {
            secret_path: Path::new("/secrets/webhook-secret"),
        };
        assert_eq!(hmac.mode(), "hmac");
        assert_eq!(Delivery::Sealed { audience: "a" }.mode(), "sealed");
    }

    #[cfg(unix)]
    #[test]
    fn the_secret_file_is_readable_by_its_owner_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("vsms-demo-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("webhook-secret");
        // A file that already exists with a wider mode is narrowed, not trusted.
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_secret_file(&path, "the-new-value").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the-new-value");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_url_means_no_endpoint_and_a_stray_mode_flag_is_an_error() {
        assert_eq!(delivery_plan(None, None, None).unwrap(), None);
        assert!(delivery_plan(None, Some("demo-app"), None).is_err());
        assert!(delivery_plan(None, None, Some(Path::new("/s"))).is_err());
    }

    #[test]
    fn a_url_needs_exactly_one_way_of_authenticating_deliveries() {
        let url = Some("http://demo-app:9000/webhooks");
        let none = delivery_plan(url, None, None).unwrap_err().to_string();
        assert!(
            none.contains("--webhook-audience") && none.contains("--webhook-secret-out"),
            "{none}"
        );
        let both = delivery_plan(url, Some("demo-app"), Some(Path::new("/s")))
            .unwrap_err()
            .to_string();
        assert!(both.contains("not both"), "{both}");
    }
}
