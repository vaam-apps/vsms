#![doc = include_str!("main.md")]

use anyhow::{bail, Context, Result};
use clap::Parser;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::FilterExpr;
use sms_api::schema::{
    app as app_filter, provider as provider_filter, sender_id as sender_id_filter,
    sender_id_registration as sender_id_registration_filter, Cratestack, CreateAppInput,
    CreateSenderIdInput, CreateSenderIdRegistrationInput, UpdateSenderIdInput,
};
use sms_api::{Principal, PrincipalKind};

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
}

/// `create` the `App` row, or resolve the id of the one that already
/// exists. Same create-then-catch-`23505` shape `sms-gateway
/// seed-dispatch`'s own `create_or_find_provider` uses: `App.slug` is
/// `@unique`.
async fn create_or_find_demo_app(
    db: &Cratestack,
    ctx: &cratestack::CoolContext,
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
    ctx: &cratestack::CoolContext,
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
                        "shortcode".to_owned()
                    } else {
                        "alphanumeric".to_owned()
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
                status: "approved".to_owned(),
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

    create_or_find_demo_app(&db, &ctx, &name, &slug).await?;
    ensure_demo_sender_id(&db, &ctx, &sender_id, &provider_row.id).await?;

    println!("demo App/SenderId fixtures ready (slug={slug:?}, sender={sender_id:?})");
    Ok(())
}
