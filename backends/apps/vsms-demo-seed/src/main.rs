//! Seeds the one `App` + approved `SenderId` a GHCR-only showcase compose
//! stack (`compose.demo.yaml`) needs before `sms-gateway provision-client`
//! can run at all.
//!
//! **This used to be `sms-gateway seed-demo-app` (`Command::SeedDemoApp`,
//! `app/sms-gateway/src/main.rs`).** Moved out into its own binary, its own
//! crate, and its own image (`ghcr.io/vymalo/vsms/demo`) — the maintainer's
//! own words on the image-hygiene PR that did this: "Images should be tiny
//! and have ONLY the core business logic. Demo stuffs, never! Unless it's
//! rust -> binary, it should never be in production images." Roughly 200
//! lines of demo-only fixture-seeding logic (this file) were compiled into
//! every production `sms-gateway` image before this — every real deployment
//! shipped code it could never legitimately run (`App`/`SenderId` create
//! policy admits only `hasRole('owner') || hasRole('admin')`, and no
//! production bootstrap path ever calls this), purely because it happened
//! to live in the same `main.rs`.
//!
//! # Why this is a Rust binary and not a shell script
//!
//! The obvious-looking alternative — a plain shell script in a tiny Alpine
//! image — was considered first and rejected, because of what this command
//! actually has to do, not because Rust is the default. `App`/`SenderId`/
//! `SenderIdRegistration` creation has to go through `CrateStack`'s generated
//! delegates (R1: "all data access goes through `CrateStack` delegates. Never
//! raw `sqlx`" — `CONTRIBUTING.md`) to get the real things this repo's own
//! §2.0 grammar table promises: `@@allow` policy enforcement (this command
//! runs under a hand-built `owner`-role [`sms_api::Principal`], the same
//! "a CLI acting on behalf of a human role" shape `sms-gateway
//! provision-user`/`record-route-validation` already use — not a rubber
//! stamp), a real `@@audit` row per write, and `cs_cuid()`-generated ids.
//! Two ways to get that from outside the `sms-gateway` binary itself:
//!
//! - **Over the HTTP API**, the way an external integrator would. Rejected:
//!   `App.create`/`SenderId.create`'s own `@@allow` admit only
//!   `hasRole('owner') || hasRole('admin')`, and `GatewayAuth::authenticate`
//!   never mints either role for a machine (`client_credentials`) token —
//!   only a real human login (#194) can, and `compose.demo.yaml`'s own
//!   dependency chain runs this seeding step *before* `provision-client`,
//!   let alone before any human account exists (`provision-user`, gated
//!   behind the `console` profile and not even started in a backend-only
//!   run). There is no bearer token this step could present that would ever
//!   be let through.
//! - **Raw `psql`**, bypassing R1 entirely. Rejected: it would skip the
//!   `@@audit` row every other write in this system gets, skip whatever
//!   `@db_enforce`-backed `CHECK` constraints exist (and silently skip the
//!   ones that don't — `@regex` is a documented no-op at the DB level,
//!   AGENTS.md's own "Framework constraints" table), and duplicate
//!   `App`/`SenderId`/`SenderIdRegistration`'s insert shape as hand-written
//!   SQL that would drift the moment the schema does. A demo tool getting
//!   this wrong fails quietly by producing fixtures the rest of the stack
//!   then rejects for reasons that look unrelated — not worth the shortcut
//!   for a handful of rows.
//!
//! So this stays exactly what it was inside `sms-gateway`: real
//! `Cratestack` delegate calls, through `sms-api`'s generated schema types,
//! under a hand-built context — just compiled into its own tiny binary and
//! its own image instead of the production one. This is the "unless it's
//! rust -> binary" carve-out the maintainer's own instruction names
//! explicitly: what's forbidden is demo logic *inside the production
//! image*, not Rust itself.
//!
//! # Demo-only — deliberately not part of any production bootstrap sequence
//!
//! A real deployment's first `App` is a business decision (quota, IP
//! allowlist, a `SenderId` actually approved by a real provider account)
//! this command cannot make on an operator's behalf; every production
//! runbook creates it by hand or through the console, once one exists
//! (#52/#58's App CRUD screen). The fixed defaults below — an
//! auto-approved `SenderIdRegistration` with no real provider approval
//! behind it — exist only to unblock the showcase. This is the GHCR-only
//! equivalent of `backends/crates/sms-api/examples/send_test_message.rs`'s own
//! fixture-seeding half; that binary is a `cargo run --example`, never
//! published as a GHCR image, so a `build:`-free compose stack has no way
//! to invoke it either.
//!
//! Idempotent, the same look-up-by-unique-key-then-reuse shape
//! `sms-gateway seed-dispatch`'s own `create_or_find_provider` already
//! uses: safe to run on every `docker compose up`. Requires the `Provider`
//! row named by `--provider-key` to already exist (i.e. `sms-gateway
//! seed-dispatch` has already run) — `SenderIdRegistration.providerId` is a
//! real foreign key.

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
