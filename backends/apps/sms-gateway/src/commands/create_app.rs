//! `Command::CreateApp` — see that variant's own doc comment in `main.rs`
//! for what this does and why it exists (`deployment.adoc`'s "Known
//! seams" / "Backend-only deployment" sections).

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::{Cratestack, CreateAppInput, app as app_filter};
use sms_api::{Principal, PrincipalKind};

/// `Command::CreateApp`'s flags. See `Command::CreateApp`'s own doc
/// comment in `main.rs` — the enum variant carries the "why", this struct
/// only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct CreateAppArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,

    /// `App.slug`'s own `@regex` — lowercase alphanumeric and `-`
    /// only, 3-40 characters.
    #[arg(long)]
    pub(crate) slug: String,

    #[arg(long)]
    pub(crate) name: String,

    /// Which of `App`'s two create-admitted roles to run this call
    /// under. Same choice, same reasoning as `ProvisionClient`'s own
    /// `--role`.
    #[arg(long, default_value = "owner")]
    pub(crate) role: String,
}

/// `create` the `App` row, or resolve the id of the one that already
/// exists — same create-then-catch-`23505` idiom as
/// `create_or_find_provider` (`commands::seed_dispatch`), mirroring
/// `backends/apps/vsms-demo-seed/src/main.rs`'s own
/// `create_or_find_demo_app` (a deliberately separate binary/image — see
/// that crate's own module doc — so the two aren't shared code, just the
/// same small pattern applied twice). Pulled out of
/// [`create_app_command`] purely to keep that function under
/// `clippy::too_many_lines`, the same reason `create_or_find_provider`
/// was split out of `seed_dispatch_core` above.
async fn create_or_find_app(
    db: &Cratestack,
    ctx: &cratestack::CratestackContext,
    slug: &str,
    name: &str,
) -> Result<String> {
    match db
        .app()
        .create(CreateAppInput {
            name: name.to_owned(),
            slug: slug.to_owned(),
            description: None,
            defaultSenderIdId: None,
            // Placeholders, not a policy decision this command makes for
            // an operator — see `Command::CreateApp`'s own doc comment.
            // Matching `vsms-demo-seed::create_or_find_demo_app`'s own
            // field choices: an unrestricted quota, no IP allowlist (the
            // empty-list sentinel encoding, §2.0), no GSM-7
            // transliteration.
            monthlyQuota: 1000,
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
        // Narrowed to `apps_slug_key` specifically (review round 1, item
        // 13) — an unrelated `23505` (a bug, not a duplicate slug) now
        // propagates loudly through the final `Err(e) => ...` arm below
        // instead of being folded into "already exists".
        Err(e)
            if e.db_sqlstate() == Some(sms_api::errors::UNIQUE_VIOLATION)
                && e.db_constraint() == Some("apps_slug_key") =>
        {
            let existing = db
                .app()
                .find_many()
                .where_expr(FilterExpr::from(app_filter::slug().eq(slug.to_owned())))
                .limit(1)
                .run(ctx)
                .await
                .context("looking up the existing App row after a duplicate-slug create")?;
            let row = existing.into_iter().next().with_context(|| {
                format!(
                    "App row with slug {slug:?} reported as a duplicate on create but not \
                     found on lookup"
                )
            })?;
            println!("App {} (slug={slug:?}) already exists", row.id);
            Ok(row.id)
        }
        Err(e) => Err(e).context("creating the App row"),
    }
}

/// `Command::CreateApp`'s body — see that variant's own doc comment for
/// what this does and why it exists. Takes [`CreateAppArgs`] directly
/// rather than the whole `Command`: `main`'s own dispatch already
/// extracted it from `Command::CreateApp` at the match site.
pub(crate) async fn create_app_command(args: CreateAppArgs) -> Result<()> {
    let CreateAppArgs {
        database_url,
        slug,
        name,
        role,
    } = args;

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — App's own @allow admits nothing else on \
             create, got {role:?}"
        );
    }

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&database_url)
        .await
        .context("connecting to Postgres")?;
    let db = Cratestack::builder(pool).build();
    let ctx = Principal {
        sub: format!("sms-gateway:create-app:{role}"),
        kind: PrincipalKind::User,
        role: role.clone(),
        app_id: String::new(),
    }
    .into_context();

    let app_id = create_or_find_app(&db, &ctx, &slug, &name).await?;
    println!("app id: {app_id}");
    Ok(())
}
