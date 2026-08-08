//! Seed the minimal fixtures `sendMessage` needs and send one real message —
//! the trigger for #36's acceptance gate (a real handset delivery, and the
//! `kill -9` lease-reclaim test).
//!
//! No admin console or `provisionAppClient` procedure exists yet (both are
//! M4), so this does by hand, once, idempotently, exactly what those would
//! eventually do: an `App` + `AppClient` to send under, and an `active`
//! `Provider` row keyed `"orange_cm"` (the row `dispatch`'s routing pass
//! and `sms-gateway`'s own `resolve_provider_row_id` both need to exist,
//! matching `OrangeCmProvider::key()` exactly — nothing here parses or
//! uses this row's `config`/`credentialRef`, both binaries construct the
//! real adapter from their own CLI flags/env instead, per §2.4).
//!
//! Does **not** seed a `SenderId` — `--sender-id` must already be an
//! Orange-approved sender name or short code from a real contract; this
//! tool has no way to know that, and getting it wrong fails at Orange's
//! own API, not here. It *does* register that sender as `approved` in this
//! database (`SenderIdRegistration`), since `sendMessage` itself checks our
//! own records, not Orange's.
//!
//! ```bash
//! DATABASE_URL=postgres://... cargo run -p sms-api --example send_test_message -- \
//!     --to +237677123456 --sender-id VYMALO --body "Test message, #36 gate"
//! ```

use anyhow::Context;
use chrono::Utc;
use clap::Parser;
use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::{CoolContext, FilterExpr};
use sms_api::auth::{Principal, PrincipalKind};
use sms_api::schema::{
    self, procedures::send_message, procedures::ProcedureRegistry, provider, sender_id,
    sender_id_registration, Cratestack,
};
use sms_api::Procedures;

/// Fixed, well-known slugs/keys — reused across repeated runs of this tool
/// rather than accumulating a fresh `App`/`Provider` row every time.
const APP_SLUG: &str = "m2-gate-app";
const CLIENT_ID: &str = "m2-gate-client";
const PROVIDER_KEY: &str = "orange_cm";

#[derive(Parser)]
#[command(about = "Seed fixtures and send one real message for #36's acceptance gate")]
struct Cli {
    #[arg(long, env = "DATABASE_URL")]
    database_url: String,
    /// E.164, the real test handset.
    #[arg(long)]
    to: String,
    /// Must already be approved on the real Orange account this
    /// deployment's credentials belong to.
    #[arg(long)]
    sender_id: String,
    #[arg(long, default_value = "Test message from the #36 acceptance gate")]
    body: String,
    /// #134: same pepper `sms-gateway serve` would need — this tool calls
    /// `sendMessage` directly rather than over HTTP, so it needs its own
    /// copy to construct a `Procedures`. See `sms_api::pepper`'s module doc.
    #[arg(long, env = "SMS_HASH_PEPPER")]
    hash_pepper: String,
}

fn owner() -> CoolContext {
    Principal {
        sub: "send-test-message-tool".to_owned(),
        kind: PrincipalKind::User,
        role: "owner".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

fn sys() -> CoolContext {
    Principal {
        sub: "send-test-message-tool".to_owned(),
        kind: PrincipalKind::App,
        role: "system".to_owned(),
        app_id: String::new(),
    }
    .into_context()
}

/// #24: `sendMessage` now gates on `require_permission(ctx, "sms:send")`
/// (Layer 2) before anything else in the procedure runs. This tool calls
/// `send_message` directly through `ProcedureRegistry`, bypassing
/// `GatewayAuth` (see this file's own module doc — no admin console or
/// `provisionAppClient` exists yet), so it has to carry the same claim a
/// real token's `scope` would by hand.
fn app_caller() -> CoolContext {
    let mut ctx = Principal {
        sub: CLIENT_ID.to_owned(),
        kind: PrincipalKind::App,
        role: "developer".to_owned(),
        app_id: String::new(),
    }
    .into_context();
    ctx.extensions.insert(
        "scope".to_owned(),
        cratestack::Value::String("sms:send".to_owned()),
    );
    ctx
}

async fn ensure_provider(db: &Cratestack) -> anyhow::Result<String> {
    let existing = db
        .provider()
        .find_many()
        .where_expr(FilterExpr::from(
            provider::key().eq(PROVIDER_KEY.to_owned()),
        ))
        .limit(1)
        .run(&owner())
        .await?;
    if let Some(row) = existing.into_iter().next() {
        if row.state == schema::ProviderState::active {
            println!("reusing existing Provider {} (key={PROVIDER_KEY})", row.id);
        } else {
            // A prior run created this row but was interrupted before
            // activating it (or an operator disabled it since) — reusing
            // it silently would leave dispatch's routing pass rejecting
            // every message with "no active provider" while this tool
            // keeps claiming the fixture is ready. Self-heal rather than
            // just report the gap.
            db.provider()
                .update(row.id.clone())
                .set(schema::UpdateProviderInput {
                    state: Some(schema::ProviderState::active),
                    ..Default::default()
                })
                .run(&owner())
                .await?;
            println!(
                "reusing existing Provider {} (key={PROVIDER_KEY}) — was {:?}, activated it",
                row.id, row.state
            );
        }
        return Ok(row.id);
    }

    let created = db
        .provider()
        .create(schema::CreateProviderInput {
            key: PROVIDER_KEY.to_owned(),
            displayName: "Orange Cameroon".to_owned(),
            kind: schema::ProviderKind::orange_cm_http,
            // Not parsed by dispatch or sms-gateway — both construct the
            // real adapter from CLI/env, not this column. See module doc.
            config: "{}".to_owned(),
            credentialRef: "env:ORANGE_CM_CLIENT_ID".to_owned(),
            maxTps: 5.0,
            maxDailySubmissions: 5000,
            supportsDlr: true,
            supportsAlphaSender: true,
            supportsUcs2: true,
            supportsConcat: true,
            costPerSegmentXaf: "19".parse()?,
            healthCheckedAt: None,
        })
        .run(&owner())
        .await?;

    db.provider()
        .update(created.id.clone())
        .set(schema::UpdateProviderInput {
            state: Some(schema::ProviderState::active),
            ..Default::default()
        })
        .run(&owner())
        .await?;

    println!(
        "created and activated Provider {} (key={PROVIDER_KEY})",
        created.id
    );
    Ok(created.id)
}

/// `sendMessage`'s own `resolve_sender_id` (`procedures.rs`) requires both
/// an `active` `SenderId` *and* an existing `approved`
/// `SenderIdRegistration` — not just one or the other. Idempotent and
/// self-healing: a prior run interrupted between creating the `SenderId`
/// and finishing its registration/activation would otherwise leave a row
/// this tool's "reuse" path trusted as ready when it wasn't.
async fn ensure_sender_ready(
    db: &Cratestack,
    sender_id_row_id: &str,
    provider_id: &str,
    already_active: bool,
) -> anyhow::Result<()> {
    let has_approved_registration = !db
        .sender_id_registration()
        .find_many()
        .where_expr(
            FilterExpr::from(sender_id_registration::senderIdId().eq(sender_id_row_id.to_owned()))
                .and(sender_id_registration::status().eq("approved".to_owned())),
        )
        .limit(1)
        .run(&owner())
        .await?
        .is_empty();

    if !has_approved_registration {
        db.sender_id_registration()
            .create(schema::CreateSenderIdRegistrationInput {
                senderIdId: sender_id_row_id.to_owned(),
                providerId: provider_id.to_owned(),
                status: "approved".to_owned(),
                submittedAt: Some(Utc::now()),
                approvedAt: Some(Utc::now()),
                reference: None,
                rejectionReason: None,
            })
            .run(&owner())
            .await?;
    }

    if !already_active {
        db.sender_id()
            .update(sender_id_row_id.to_owned())
            .set(schema::UpdateSenderIdInput {
                active: Some(true),
                ..Default::default()
            })
            .run(&owner())
            .await?;
    }
    Ok(())
}

async fn ensure_approved_sender(
    db: &Cratestack,
    value: &str,
    provider_id: &str,
) -> anyhow::Result<String> {
    let existing = db
        .sender_id()
        .find_many()
        .where_expr(FilterExpr::from(sender_id::value().eq(value.to_owned())))
        .limit(1)
        .run(&owner())
        .await?;
    if let Some(row) = existing.into_iter().next() {
        ensure_sender_ready(db, &row.id, provider_id, row.active).await?;
        println!("reusing existing SenderId {:?} ({})", row.value, row.id);
        return Ok(row.value);
    }

    let created = db
        .sender_id()
        .create(schema::CreateSenderIdInput {
            value: value.to_owned(),
            kind: if value.chars().all(|c| c.is_ascii_digit()) {
                "shortcode".to_owned()
            } else {
                "alphanumeric".to_owned()
            },
            notes: Some("seeded by send_test_message for the #36 acceptance gate".to_owned()),
        })
        .run(&owner())
        .await?;

    ensure_sender_ready(db, &created.id, provider_id, false).await?;

    println!(
        "created, registered and activated SenderId {:?} ({}) — must already be approved on the \
         real Orange account, or the real send will fail at Orange's own API",
        created.value, created.id
    );
    Ok(created.value)
}

async fn ensure_app_and_client(db: &Cratestack) -> anyhow::Result<()> {
    let app = db
        .app()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::app::slug().eq(APP_SLUG.to_owned()),
        ))
        .limit(1)
        .run(&owner())
        .await?;
    let app_id = if let Some(row) = app.into_iter().next() {
        println!("reusing existing App {} (slug={APP_SLUG})", row.id);
        row.id
    } else {
        let created = db
            .app()
            .create(schema::CreateAppInput {
                name: "M2 gate test app".to_owned(),
                slug: APP_SLUG.to_owned(),
                description: Some("#36 acceptance gate — real handset delivery".to_owned()),
                defaultSenderIdId: None,
                monthlyQuota: 1000,
                ipAllowlist: " ".to_owned(),
                transliterateToGsm7: false,
                deletedAt: None,
            })
            .run(&owner())
            .await?;
        println!("created App {} (slug={APP_SLUG})", created.id);
        created.id
    };

    let client = db
        .app_client()
        .find_many()
        .where_expr(FilterExpr::from(
            schema::app_client::clientId().eq(CLIENT_ID.to_owned()),
        ))
        .limit(1)
        .run(&sys())
        .await?;
    if client.into_iter().next().is_some() {
        println!("reusing existing AppClient {CLIENT_ID:?}");
    } else {
        db.app_client()
            .create(schema::CreateAppClientInput {
                appId: app_id,
                clientId: CLIENT_ID.to_owned(),
                label: "m2 gate test client".to_owned(),
                scopes: " sms:send ".to_owned(),
                lastUsedAt: None,
                retiredAt: None,
            })
            .run(&sys())
            .await?;
        println!("created AppClient {CLIENT_ID:?}");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let pepper = sms_api::HashPepper::new(cli.hash_pepper.clone())
        .context("SMS_HASH_PEPPER is invalid — see sms_api::pepper's module doc")?;

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&cli.database_url)
        .await?;
    let db = Cratestack::builder(pool).build();

    let provider_id = ensure_provider(&db).await?;
    let sender_value = ensure_approved_sender(&db, &cli.sender_id, &provider_id).await?;
    ensure_app_and_client(&db).await?;

    let procedures = Procedures::new(pepper);
    let result = procedures
        .send_message(
            &db,
            &app_caller(),
            send_message::Args {
                args: schema::SendMessageInput {
                    to: cli.to.clone(),
                    body: cli.body.clone(),
                    senderId: Some(sender_value),
                    class: None,
                    clientRef: None,
                    scheduledAt: None,
                    validityMinutes: None,
                },
            },
        )
        .await?;

    println!();
    println!("message id:       {}", result.messageId);
    println!("state:             {:?}", result.state);
    println!("operator:          {:?}", result.operator);
    println!(
        "encoding/segments: {:?} / {}",
        result.encoding, result.segments
    );
    println!();
    println!("Watch it move (needs dispatch running against the same DATABASE_URL):");
    println!(
        "  psql \"$DATABASE_URL\" -c \"SELECT state, provider_message_ref, state_reason FROM \
         messages WHERE id = '{}';\"",
        result.messageId
    );
    Ok(())
}
