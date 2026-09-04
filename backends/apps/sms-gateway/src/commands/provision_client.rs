//! `Command::ProvisionClient` — see that variant's own doc comment in
//! `main.rs` for what this does and why it exists (#137).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cratestack::FilterExpr;
use cratestack::sqlx::postgres::PgPoolOptions;
use sms_api::schema::procedures::{ProcedureRegistry, provision_app_client};
use sms_api::schema::{Cratestack, ProvisionClientInput, app as app_filter};
use sms_api::{Principal, PrincipalKind, Procedures};

/// `Command::ProvisionClient`'s flags. See `Command::ProvisionClient`'s
/// own doc comment in `main.rs` — the enum variant carries the "why",
/// this struct only carries the flags themselves.
#[derive(Debug, clap::Args)]
pub(crate) struct ProvisionClientArgs {
    #[arg(long, env = "DATABASE_URL")]
    pub(crate) database_url: String,

    /// The `App.id` this client acts on behalf of. Must already exist
    /// and be active — `provision_client` checks both and refuses
    /// otherwise. Exactly one of `--app-id`/`--app-slug` is required.
    #[arg(
        long,
        required_unless_present = "app_slug",
        conflicts_with = "app_slug"
    )]
    pub(crate) app_id: Option<String>,

    /// The `App.slug` this client acts on behalf of, resolved to its
    /// id before provisioning — the compose-only equivalent of passing
    /// `--app-id` by hand off a `psql` query (`getting-started.md`'s
    /// own step 7). Added so `compose.demo.yaml`'s `provision-client`
    /// one-shot never has to shuttle an id computed by an earlier
    /// one-shot (`vsms-demo-seed`, a separate demo-only binary/image —
    /// see `backends/apps/vsms-demo-seed/src/main.rs`'s own module doc) between
    /// two separate containers — both commands agree on the same
    /// well-known slug instead.
    #[arg(long, required_unless_present = "app_id", conflicts_with = "app_id")]
    pub(crate) app_slug: Option<String>,

    /// A human-readable label for the resulting `AppClient`, e.g.
    /// `"admin console"` or `"otp sender"`.
    #[arg(long)]
    pub(crate) label: String,

    /// One or more scopes to provision the client with, e.g.
    /// `--scope sms:send --scope sms:read`. At least one is required —
    /// an unscoped client can authenticate but can call nothing.
    #[arg(long = "scope", required = true)]
    pub(crate) scopes: Vec<String>,

    /// Which of `provisionAppClient`'s two admitted roles to run the
    /// call under. Both are equally privileged for this call; `owner`
    /// is the default because it's the role every existing live test
    /// already provisions under (`m1_acceptance_gate_live_postgres.rs`,
    /// `provision_app_client_live_postgres.rs`).
    #[arg(long, default_value = "owner")]
    pub(crate) role: String,

    /// Where to write the returned private key, PEM-encoded. Created
    /// with `0600` permissions; this command refuses to run if the
    /// path already exists rather than silently overwriting a key
    /// someone may still be using.
    #[arg(long)]
    pub(crate) key_out: PathBuf,

    /// Optional: also write the plain-text `clientId` to this path —
    /// `provisionAppClient` generates it server-side (a random
    /// `appc_<uuid>`, never caller-chosen), so there is otherwise no
    /// way for a second, separate container to learn the value this
    /// specific run produced without re-parsing stdout. Not sensitive
    /// the way `--key-out`'s contents are (it's the public half of the
    /// credential — every `/token` request already sends it in the
    /// clear as the assertion's own `sub`/`iss`), so this is written
    /// with ordinary create-or-truncate semantics and no restrictive
    /// mode, unlike `--key-out`. `compose.demo.yaml`'s own
    /// `provision-client` step is the intended caller: it feeds the
    /// admin container's `SMS_CONSOLE_CLIENT_ID` env var from this
    /// file's contents, since compose has no other way to pass one
    /// container's computed output into a sibling container's
    /// environment.
    #[arg(long)]
    pub(crate) client_id_out: Option<PathBuf>,

    /// #134: `Procedures::new` now requires a `HashPepper` unconditionally,
    /// even though `provision_app_client` itself never hashes anything —
    /// only `sendMessage` does. Same flag name and env var as `Serve`'s
    /// own `--hash-pepper`/`SMS_HASH_PEPPER`, so an operator running
    /// this alongside `serve` supplies the identical value once via
    /// their environment rather than learning two different names for
    /// the same secret.
    #[arg(long, env = "SMS_HASH_PEPPER")]
    pub(crate) hash_pepper: String,
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
    use std::io::Write as _;

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

/// `Command::ProvisionClient`'s body, pulled out of `main`'s own `match`
/// purely to stay under `clippy::too_many_lines` — see that variant's own
/// doc comment for what this does and why it exists. Takes
/// [`ProvisionClientArgs`] directly rather than the whole `Command`: `main`'s
/// own dispatch already extracted it from `Command::ProvisionClient` at
/// the match site, so there is no `unreachable!()` guard to carry here any
/// more.
pub(crate) async fn provision_client_command(args: ProvisionClientArgs) -> Result<()> {
    let ProvisionClientArgs {
        database_url,
        app_id,
        app_slug,
        label,
        scopes,
        role,
        key_out,
        client_id_out,
        hash_pepper,
    } = args;

    if role != "owner" && role != "admin" {
        bail!(
            "--role must be \"owner\" or \"admin\" — provisionAppClient's own @allow admits \
             nothing else, got {role:?}"
        );
    }
    // An empty string satisfies clap's `required_unless_present`/
    // `conflicts_with` XOR just as well as a real value would — found
    // live (review round 1, blocker 2): `deploy/docker-compose.yml`'s own
    // `provision-console-client` one-shot service reads `--app-id` from
    // `${SMS_CONSOLE_APP_ID:-}`, which resolves to an empty string for
    // every `docker compose up` that isn't specifically provisioning a
    // console client (Compose interpolates every service's fields up
    // front, regardless of profile — the same reason every other
    // console-only var in this file uses `:-` rather than `:?`). Refuse
    // with a clear message rather than letting an empty id reach a real
    // `provisionAppClient` call and fail with a confusing "App not
    // found".
    if app_id.as_deref() == Some("") || app_slug.as_deref() == Some("") {
        bail!(
            "--app-id/--app-slug must not be empty — pass a real App id (see `sms-gateway \
             create-app`) via SMS_CONSOLE_APP_ID or the flag directly"
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

    // `clap`'s `required_unless_present`/`conflicts_with` on the two
    // `Command::ProvisionClient` fields already guarantee exactly one of
    // `app_id`/`app_slug` is `Some` by the time this runs — this resolves
    // the slug case to the id `provisionAppClient` actually takes, so
    // `compose.demo.yaml`'s own `provision-client` one-shot never has to
    // learn an id the `seed-demo-app` compose service (its image is
    // `vsms-demo-seed`, a separate demo-only binary — no `SeedDemoApp`
    // variant exists in this file any more) computed in a separate
    // container.
    let app_id = if let Some(app_id) = app_id {
        app_id
    } else {
        let slug = app_slug
            .expect("clap's required_unless_present/conflicts_with guarantees app_id xor app_slug");
        db.app()
            .find_many()
            .where_expr(FilterExpr::from(app_filter::slug().eq(slug.clone())))
            .limit(1)
            .run(&ctx)
            .await
            .context("resolving --app-slug to an App id")?
            .into_iter()
            .next()
            .with_context(|| format!("no App with slug {slug:?} exists"))?
            .id
    };

    let procedures = Procedures::new(pepper);
    let args = provision_app_client::Args {
        args: ProvisionClientInput {
            appId: app_id,
            label,
            scopes,
        },
    };
    // cratestack 0.7.13 (cratestack#512): calling `procedures.provision_app_client(&db,
    // &ctx, args)` directly used to skip `@allow` entirely — the `--role`
    // check above already enforced the same "owner or admin" policy by
    // hand, so this was always a redundant guard rather than a live gap,
    // but the framework no longer offers the 3-argument shape at all.
    // `invoke_with_db` is "the sanctioned way to invoke a procedure from
    // non-HTTP code" per its own doc comment — it runs the real
    // `authorize_with_db` (so this CLI command now genuinely enforces
    // `provisionAppClient`'s policy, not just this file's own copy of it)
    // and hands the resulting `Authorized` witness into the trait method.
    let provisioned = provision_app_client::invoke_with_db(&db, &args, &ctx, |authorized| {
        procedures.provision_app_client(&db, &ctx, args.clone(), authorized)
    })
    .await
    .context("provisioning the client")?;

    // Destructured immediately and never reassembled: nothing past this
    // point may hold, log, or `{:?}`-print `provisioned` as a whole — see
    // `write_private_key_pem`'s own doc for why the file below is the
    // only place this value's private key is ever allowed to land.
    let client_id = provisioned.clientId;
    let private_key_pem = provisioned.privateKeyPem;

    write_private_key_pem(&key_out, &private_key_pem)?;

    if let Some(client_id_out) = &client_id_out {
        std::fs::write(client_id_out, &client_id)
            .with_context(|| format!("writing the client id to {}", client_id_out.display()))?;
    }

    println!("provisioned client: {client_id}");
    println!("private key written to: {}", key_out.display());
    if let Some(client_id_out) = &client_id_out {
        println!("client id written to: {}", client_id_out.display());
    }
    println!();
    println!("paste into the console (or any other machine caller)'s environment:");
    println!("  SMS_CONSOLE_CLIENT_ID={client_id}");
    println!("  SMS_CONSOLE_PRIVATE_KEY_PATH={}", key_out.display());
    Ok(())
}
