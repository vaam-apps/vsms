//! `Command::SeedDispatch` — see that variant's own doc comment in
//! `main.rs` for what this seeds and why it's a hard rename from the old
//! `seed-provider` (#62/#148).

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{
    Cratestack, CreateProviderInput, CreateRouteInput, ProviderKind, ProviderState,
    UpdateProviderInput, UpdateRouteInput, provider as provider_filter, route as route_filter,
};
use sms_api::{Principal, PrincipalKind};

/// `Command::SeedDispatch`'s flags. See `Command::SeedDispatch`'s own doc
/// comment in `main.rs` — the enum variant carries the "why", this struct
/// only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct SeedDispatchArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,

    /// Must match `SmsProvider::key()` for whichever adapter is
    /// actually configured — `resolve_provider_row_id` looks the row
    /// up by exactly this key. `orange_cm` is the only adapter with a
    /// real implementation (`sms-provider-orange-cm`) as of this
    /// milestone, matching `Serve`'s own hard-coded Orange wiring.
    #[arg(long, default_value = "orange_cm")]
    pub(crate) key: String,

    #[arg(long, default_value = "Orange Cameroon SMS API")]
    pub(crate) display_name: String,

    /// One of `ProviderKind`'s own variants (`schema.cstack`):
    /// `orange_cm_http`, `mtn_http`, `aggregator_http`, `smpp`.
    #[arg(long, default_value = "orange_cm_http")]
    pub(crate) kind: String,

    /// Never read by `sms-gateway` or `sms-worker` to construct the
    /// real adapter — both build it from their own flags/env instead
    /// (§2.4), confirmed against `send_test_message.rs`'s own doc
    /// comment. This row's job is only to exist, carry the right
    /// `key`, and end up `state = 'active'`.
    #[arg(long, default_value = "{}")]
    pub(crate) config: String,

    #[arg(long, default_value = "env:ORANGE_CM_CLIENT_SECRET")]
    pub(crate) credential_ref: String,

    #[arg(long, default_value_t = 10.0)]
    pub(crate) max_tps: f64,

    #[arg(long, default_value_t = 100_000)]
    pub(crate) max_daily_submissions: i64,

    /// Parsed as a `cratestack::Decimal`, not a float — money stays
    /// fixed-point throughout this codebase (§2.0).
    #[arg(long, default_value = "0")]
    pub(crate) cost_per_segment_xaf: String,

    /// Which of `Provider`'s two create-admitted roles to run this
    /// call under. Same choice, same reasoning as `ProvisionClient`'s
    /// own `--role`: `owner` is the default because every existing
    /// live Provider fixture in this repo already writes under it.
    #[arg(long, default_value = "owner")]
    pub(crate) role: String,
}

/// `ProviderKind`'s variants aren't `clap::ValueEnum` (it's a type generated
/// by `include_server_schema!` in a downstream crate, not one this binary
/// can derive a foreign trait on), so `--kind` stays a plain `String` and is
/// matched by hand here — pulled out of [`seed_dispatch_command`] purely to
/// keep that function under `clippy::too_many_lines`, the same reason
/// `rotate_signing_key_command`/`provision_client_command` were already
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
    ctx: &cratestack::CratestackContext,
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
    ctx: &cratestack::CratestackContext,
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
/// above are. Takes [`SeedDispatchArgs`] directly rather than the whole
/// `Command`: `main`'s own dispatch already extracted it from
/// `Command::SeedDispatch` at the match site.
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
pub(crate) async fn seed_dispatch_command(args: SeedDispatchArgs) -> Result<()> {
    let SeedDispatchArgs {
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
    } = args;

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

    seed_dispatch_core(
        &db,
        &ctx,
        CreateProviderInput {
            key,
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
            circuitOpenUntil: None,
        },
    )
    .await
}

/// The actual `Provider` + catch-all `Route` seeding logic, shared by
/// [`seed_dispatch_command`] and `bootstrap_command` — pulled out so
/// `bootstrap` reuses this exact function over its own already-open pool
/// rather than re-deriving `Command::SeedDispatch`'s field defaults a
/// second time or opening a second connection just to run this step.
pub(crate) async fn seed_dispatch_core(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    input: CreateProviderInput,
) -> Result<()> {
    let key = input.key.clone();
    let (provider_id, provider_version, already_active) =
        create_or_find_provider(db, ctx, &key, input).await?;

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
            .run(ctx)
            .await
            .context("activating the Provider row")?;
        println!("activated Provider {provider_id} (key={key:?})");
    }

    ensure_catch_all_route(db, ctx, &provider_id).await
}
