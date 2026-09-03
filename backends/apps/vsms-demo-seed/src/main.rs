#![doc = include_str!("main.md")]

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateAppInput, CreateSenderIdInput, CreateSenderIdRegistrationInput,
    CreateWebhookEndpointInput, SenderIdKind, SenderIdRegistrationStatus, UpdateSenderIdInput,
    app as app_filter, provider as provider_filter, sender_id as sender_id_filter,
    sender_id_registration as sender_id_registration_filter,
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
    #[arg(long, env = "DATABASE_URL")]
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
    /// mounted at this address by both `compose.dev.yaml` and
    /// `compose.demo.yaml`'s `demo-app` service. `WebhookEndpoint.url`
    /// (`@uri`) has no reachability check at write time — this only has
    /// to resolve once `hooks` actually tries to deliver to it, well
    /// after this command exits.
    #[arg(long, default_value = "http://demo-app:9000/webhooks")]
    webhook_url: String,

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

    /// Where to write the signing secret this command ends up using —
    /// freshly generated (`sms_webhook::generate_secret()`) on first run,
    /// or the existing row's own `secret` on every rerun against an
    /// already-seeded database. `demo-app` has no other way to learn this
    /// value: `WebhookEndpoint.secret` is server-generated here, exactly
    /// the "one container's computed output, a sibling container's
    /// input" problem `provision-client`'s own `--client-id-out` already
    /// solves the same way. Ordinary create-or-truncate semantics, no
    /// restrictive mode — same reasoning `--client-id-out`'s own doc
    /// comment gives (`backends/apps/sms-gateway/src/main.rs`): not the
    /// half of a credential that authenticates a caller, and this whole
    /// stack is a disposable, obviously-demo deployment regardless (see
    /// `SMS_HASH_PEPPER`'s own inline compose comments).
    #[arg(long)]
    webhook_secret_out: Option<PathBuf>,
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

/// Ensure a `WebhookEndpoint` pointed at the demo app's own receiver
/// exists, and return the signing secret it currently uses — freshly
/// generated on the row this call creates, or read straight back off an
/// already-existing row on a rerun. `WebhookEndpoint` carries no unique
/// constraint on `(appId, url)` (unlike `App.slug`), so idempotency here
/// is read-then-create, the same accepted-TOCTOU-window shape
/// `ensure_demo_sender_id`'s own approved-registration check already
/// uses — safe for the same reason: this only ever runs from one
/// container in one sequential compose dependency chain, never
/// concurrently with itself.
///
/// Deliberately does NOT try to keep an existing row's `secret` in sync
/// with a caller-supplied `--webhook-secret-out` value from a *previous*
/// run that used a different generated secret — the row's own `secret`
/// is always the one written back to `webhook_secret_out`, so `demo-app`
/// and the database never disagree about which value is current, even
/// across a restart that skips re-creation.
async fn ensure_demo_webhook_endpoint(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    app_id: &str,
    url: &str,
    max_attempts: i64,
    mask_recipient: bool,
) -> Result<String> {
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
        return Ok(row.secret);
    }

    let event_types =
        sms_core::pack(DEMO_WEBHOOK_EVENT_TYPES.iter().copied()).with_context(|| {
            format!(
                "packing DEMO_WEBHOOK_EVENT_TYPES ({DEMO_WEBHOOK_EVENT_TYPES:?}) — a literal, \
                 space-free array, should never fail to pack"
            )
        })?;
    let secret = sms_webhook::generate_secret();

    let created = db
        .webhook_endpoint()
        .create(CreateWebhookEndpointInput {
            appId: app_id.to_owned(),
            url: url.to_owned(),
            eventTypes: event_types,
            secret: secret.clone(),
            prevSecret: None,
            secretRotatedAt: None,
            maskRecipient: mask_recipient,
            maxAttempts: max_attempts,
            circuitOpenUntil: None,
        })
        .run(ctx)
        .await
        .context("creating the WebhookEndpoint row")?;
    println!("created WebhookEndpoint {} (url={url:?})", created.id);

    Ok(created.secret)
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
        webhook_secret_out,
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
    let webhook_secret = ensure_demo_webhook_endpoint(
        &db,
        &ctx,
        &app_id,
        &webhook_url,
        webhook_max_attempts,
        webhook_mask_recipient,
    )
    .await?;

    if let Some(path) = &webhook_secret_out {
        std::fs::write(path, &webhook_secret)
            .with_context(|| format!("writing the webhook secret to {}", path.display()))?;
        println!("webhook secret written to: {}", path.display());
    } else {
        // No sibling container told to read a file — print it so a human
        // running this by hand can still configure a receiver.
        println!("webhook secret (no --webhook-secret-out given): {webhook_secret}");
    }

    println!(
        "demo App/SenderId/WebhookEndpoint fixtures ready (slug={slug:?}, sender={sender_id:?}, webhook={webhook_url:?})"
    );
    Ok(())
}
