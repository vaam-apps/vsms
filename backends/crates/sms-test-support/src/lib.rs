#![doc = include_str!("lib.md")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::sqlx::{AssertSqlSafe, query, query_scalar};
use tokio::process::Command;
use tokio::sync::OnceCell;

/// The Compose project name this module reserves for itself — passed as
/// `docker compose -p` on every invocation, never the root `compose.yml`'s
/// own default project name (`vsms`), so a developer's own plain
/// `docker compose up` against that same file can never collide with, or be
/// torn down by, this harness. See this module's own "Migrating from the
/// old `docker run` design" doc section for the full reasoning.
const COMPOSE_PROJECT: &str = "vsms-test-harness";

/// The one service in the root `compose.yml` this module ever starts.
const COMPOSE_SERVICE: &str = "postgres";

/// The exact, literal container name the pre-Compose version of this module
/// gave its container (`docker run --name vsms-test-harness-postgres ...`).
/// Kept only as a one-time migration target — see [`ensure_running`] — and
/// never widened into a pattern or a label filter: a container carrying
/// this exact name can only be a leftover from that specific old design,
/// never something a developer or another tool created.
const LEGACY_CONTAINER_NAME: &str = "vsms-test-harness-postgres";

/// Bound to loopback only — never `0.0.0.0` — so this is never reachable
/// from outside the machine running the tests. `compose.yml`'s own port
/// mapping spells out `127.0.0.1:` explicitly for this reason; see the
/// comment at that call site. Deliberately not `5432`: this workspace's own
/// module docs (e.g. `backends/crates/sms-worker/tests/live_postgres.rs`) tell a
/// human to run their own `docker run -p 5432:5432 postgres:16` for ad hoc
/// use, and `compose.yml`'s own default (also `5432`, for a bare
/// `docker compose up`) is a second reason this harness must never collide
/// with either.
const HOST_PORT: u16 = 55432;

/// `compose.yml`'s own hardcoded Postgres credentials — dev/test-only, not
/// secret, matching the file itself.
const DB_USER: &str = "vsms";
const DB_PASSWORD: &str = "vsms";

/// `(classid, objid)` for the advisory lock guarding the whole
/// ensure-template-migrated / drop-and-recreate-this-binary's-database
/// sequence — namespaced distinctly from `sms-worker/src/lease.rs`'s own
/// `NS` (`"SMS\0"`) so the two can never collide even though both use the
/// 2-argument form, which Postgres folds into the same 64-bit keyspace a
/// 1-argument `pg_advisory_lock(bigint)` call would also use.
const MIGRATION_LOCK_NS: i32 = 0x5654_5300; // "VTS\0" — Vsms Test Support
const MIGRATION_LOCK_KEY: i32 = 1;

/// The shared template every per-binary database is created from via
/// `CREATE DATABASE ... TEMPLATE`. (Re)migrated whenever its stamped
/// [`migrations_fingerprint`] no longer matches the working tree's (see
/// [`ensure_binary_database`]) — never queried or written to directly by
/// any test.
const TEMPLATE_DB_NAME: &str = "vsms_template";

/// Prefix for every per-binary scratch database name, so `psql -l` /
/// `pg_database` reads clearly distinguish this harness's own databases
/// (this prefix plus [`TEMPLATE_DB_NAME`]) from the container's own
/// `POSTGRES_DB=vsms` default database, which no test connects to once
/// this module's per-binary-database logic is in play.
const DB_NAME_PREFIX: &str = "vsms_test_";

/// An escape hatch for a developer (or a future CI job) who already has a
/// suitable Postgres reachable and would rather point every live suite at
/// it than have this module manage a container at all. When set, no
/// `docker` command ever runs — but the migration check below still does,
/// since a hand-provided database is not guaranteed to be migrated yet.
const OVERRIDE_URL_VAR: &str = "VSMS_TEST_DATABASE_URL";

/// Cached for the lifetime of this process — the *first* call in a test
/// binary does the (possibly slow, container-starting) work; every later
/// call in the same binary, and every call in the same test function via
/// `ca653a1`'s own per-binary `TEST_MUTEX`, is a clone of an owned
/// `String`. Deliberately not wrapped in anything that tries to clean up on
/// drop — see this module's top-level doc for why that was the previous
/// attempt's actual bug, not a detail to reproduce.
static DATABASE_URL: OnceCell<String> = OnceCell::const_new();

/// Returns a connection URL for a fully migrated Postgres, starting or
/// reusing the shared test-harness container as needed.
///
/// Every one of this workspace's `db()` test helpers should call this
/// instead of reading `DATABASE_URL` from the environment directly. Safe to
/// call from every test in every live suite: idempotent within a process
/// (the container/migration work happens at most once, on the first call)
/// and self-healing across processes (see the module doc).
///
/// # Panics
///
/// Panics with a clear message if `docker` is not on `PATH`, the daemon is
/// unreachable, the container never becomes ready within 30s, or migration
/// application fails. There is no reasonable fallback for any of these in a
/// test harness — better to fail loudly at the first `db().await` than to
/// hand back a URL that silently doesn't work.
pub async fn database_url() -> String {
    install_default_crypto_provider();
    DATABASE_URL
        .get_or_init(|| async { ensure_postgres().await })
        .await
        .clone()
}

/// Installs `ring` as the process-wide default `rustls` `CryptoProvider`,
/// for test binaries — see this module's own "the rustls crypto provider
/// gap" doc section for why a *test* binary needs this call at all when
/// every production binary's `main` already makes it.
///
/// `ring`, not `aws-lc-rs`: the panic message `reqwest` 0.13.4 prints
/// suggests `rustls::crypto::aws_lc_rs::default_provider()` — following
/// that suggestion would reintroduce the exact dependency this workspace
/// dropped (`AGENTS.md`'s "rustls, musl, and distroless" section) rather
/// than matching it. `ring` is what both binaries' own
/// `install_default_crypto_provider` install, and what every other TLS
/// consumer in this tree already resolves.
///
/// Idempotent by construction, not by a `Once`/`OnceLock`: `install_default`
/// returns `Err` if a provider is already installed (by an earlier test in
/// this binary, by this same function running again — [`database_url`]
/// calls it on every invocation, not just the first — or, in principle, by
/// `reqwest` 0.12's own lazy install happening first), and that is never a
/// reason to fail a test. `let _ =`, not `.unwrap()` or `.expect(...)`,
/// exactly like both binaries' own copies of this call.
pub fn install_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

async fn ensure_postgres() -> String {
    let base_url = if let Ok(overridden) = std::env::var(OVERRIDE_URL_VAR) {
        overridden
    } else {
        ensure_running().await;
        wait_until_reachable(&container_url()).await;
        container_url()
    };
    ensure_binary_database(&base_url).await
}

fn container_url() -> String {
    format!("postgres://{DB_USER}:{DB_PASSWORD}@127.0.0.1:{HOST_PORT}/vsms")
}

/// Absolute path to the root `compose.yml`, resolved relative to this
/// crate's own manifest directory rather than the current working directory
/// — the same reasoning [`migrations_root`] already documents for the
/// analogous migrations path, and for the same reason: this crate is
/// workspace-internal, and `cargo test` may be invoked from any directory.
fn compose_file_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../compose.yml")
        .canonicalize()
        .expect(
            "compose.yml must exist at the repository root, three levels up from \
             backends/crates/sms-test-support — has the workspace layout changed?",
        )
}

/// Builds a `docker compose -p COMPOSE_PROJECT -f <compose.yml>` command,
/// the common prefix every invocation below shares.
fn compose_command() -> Command {
    let mut cmd = Command::new("docker");
    cmd.arg("compose")
        .arg("-p")
        .arg(COMPOSE_PROJECT)
        .arg("-f")
        .arg(compose_file_path());
    cmd
}

/// Makes sure [`COMPOSE_SERVICE`] is running under [`COMPOSE_PROJECT`],
/// starting (or reviving) it via `docker compose up -d --wait` if not.
///
/// No sweep-before-create the old `docker run`-based design needed: Compose
/// itself revives a stopped container with matching config, or recreates
/// one whose config drifted, rather than erroring the way `docker run
/// --name` did against any existing container regardless of its state. The
/// one sweep this still performs is a one-time migration step, scoped to
/// [`LEGACY_CONTAINER_NAME`] by its exact name — see this module's own
/// "Migrating from the old `docker run` design" doc section.
async fn ensure_running() {
    if compose_service_running().await {
        return;
    }

    remove_legacy_container().await;

    let output = compose_command()
        .args([
            "up",
            "-d",
            "--wait",
            "--wait-timeout",
            "30",
            COMPOSE_SERVICE,
        ])
        .env("VSMS_POSTGRES_PORT", HOST_PORT.to_string())
        .output()
        .await
        .expect("spawning `docker compose up` — is Docker installed and on PATH?");

    if !output.status.success() {
        // Unlike the old `docker run --name`, `docker compose up` gives no
        // single distinguishable error string for "another process won the
        // race to start this same service" — it's designed to be
        // idempotent, so a losing race is more often silent than an error
        // at all. The reliable way to tell "we lost a race, and the winner
        // succeeded" from "this genuinely failed" is to check the outcome
        // rather than parse `stderr`.
        assert!(
            compose_service_running().await,
            "docker compose up failed to start {COMPOSE_SERVICE} under project {COMPOSE_PROJECT}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// `true` iff [`COMPOSE_SERVICE`] has a container currently in the
/// `running` state under [`COMPOSE_PROJECT`] — scoped to that project by
/// construction (`docker compose ps` only ever reports containers it
/// created under the `-p` project it was given), never by a label filter
/// that would need to be kept in sync with what actually gets created.
async fn compose_service_running() -> bool {
    let output = compose_command()
        .args(["ps", "-q", "--status", "running", COMPOSE_SERVICE])
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => !String::from_utf8_lossy(&o.stdout).trim().is_empty(),
        _ => false,
    }
}

/// Force-removes a container named exactly [`LEGACY_CONTAINER_NAME`], if
/// one exists — the pre-Compose version of this module's own fixed `docker
/// run --name`, which would otherwise still be holding [`HOST_PORT`] and
/// block Compose's own, differently-named container from binding it.
///
/// Scoped by exact literal name, never a pattern or a label filter: this is
/// the one piece of this migration step that must never be loosened, the
/// same reasoning the old design's own label-based sweep was built on — a
/// name-substring or image-based sweep on a developer's machine can destroy
/// containers this workspace did not create. Best-effort: `rm -f` on a name
/// that doesn't exist (the common case, once every machine has migrated)
/// exits non-zero, which is never a reason to fail a test.
async fn remove_legacy_container() {
    let _ = Command::new("docker")
        .args(["rm", "-f", LEGACY_CONTAINER_NAME])
        .output()
        .await;
}

/// Retries a real connection attempt rather than just polling the TCP port
/// — Postgres in the official image accepts TCP connections briefly during
/// its own internal restart-after-initdb before it's actually ready to
/// serve, and a bare TCP check would race that window.
async fn wait_until_reachable(url: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match PgPoolOptions::new().max_connections(1).connect(url).await {
            Ok(pool) => {
                pool.close().await;
                return;
            }
            Err(source) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "Postgres at {url} never became reachable within 30s: {source}"
                );
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

/// Returns `url` with its database-name path segment replaced by `db`,
/// preserving scheme, credentials, host, port, and any query string. Hand-
/// rolled rather than pulling in a URL-parsing crate for this one call
/// site: every URL this module ever builds or receives (the container's
/// own fixed-shape URL, or a developer-supplied [`OVERRIDE_URL_VAR`]) is a
/// standard `postgres://user:pass@host:port/dbname[?query]` connection
/// string, and the only thing ever needed is a textual swap of the last
/// path segment.
fn set_database_name(url: &str, db: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };
    let scheme_end = base.find("://").map_or(0, |p| p + 3);
    let slash = base[scheme_end..].find('/').map_or_else(
        || panic!("connection URL has no database-name path segment: {url}"),
        |p| p + scheme_end,
    );

    let mut result = String::with_capacity(slash + 1 + db.len());
    result.push_str(&base[..=slash]);
    result.push_str(db);
    if let Some(q) = query {
        result.push('?');
        result.push_str(q);
    }
    result
}

/// Derives this test binary's own scratch-database name from its
/// executable's own file name, so reruns of the same already-built binary
/// agree on the same name — see this module's "one database per test
/// binary" doc section for why determinism (not a random or
/// per-process-unique name) is the point.
///
/// Keeps Cargo's own `-<hex-metadata-hash>` suffix (e.g.
/// `live_postgres-8d453e5151bd11d1`) rather than stripping it. An earlier
/// version of this function stripped it, reasoning that the hash "changes
/// whenever the binary's own content does" and a stable name should
/// survive a rebuild. That reasoning produced a real collision, found live
/// by inspecting `psql -l` inside the harness container rather than by
/// code review: `backends/crates/sms-auth/tests/live_postgres.rs` and
/// `backends/crates/sms-worker/tests/live_postgres.rs` are two genuinely different
/// test binaries — different packages — that happen to share the same
/// file name. Cargo's own `target/debug/deps/` directory is flat and
/// workspace-wide (no per-package subdirectory), and the hash suffix is
/// the *only* signal in the executable's path that tells them apart —
/// confirmed empirically: the two really do get different hashes
/// (`live_postgres-8d453e5151bd11d1` vs `live_postgres-e79e21cf68671d0b`
/// in one observed build). Stripping it silently reunited them onto one
/// shared database, reintroducing the exact cross-binary interference this
/// whole module exists to eliminate. Keeping the hash trades away "the
/// same name survives an arbitrary future rebuild" (a rebuild that changes
/// the hash mints a new, differently-named database and leaves the old one
/// as inert clutter in the container — cleaned up like any other harness
/// state by `just test-live-clean`) for actual correctness within the
/// scenario that matters: two consecutive `just test-live` runs against
/// the same compiled binaries, which is what this task's own verification
/// requires and what CI's single `cargo test --workspace -- --ignored`
/// invocation always is.
fn binary_database_name() -> String {
    let exe_path =
        std::env::current_exe().expect("resolving this test binary's own executable path");
    let file_stem = exe_path
        .file_name()
        .expect("an executable path has a file name")
        .to_string_lossy()
        .into_owned();

    let mut sanitized: String = file_stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if !sanitized.starts_with(|c: char| c.is_ascii_alphabetic()) {
        sanitized = format!("t_{sanitized}");
    }

    let mut name = format!("{DB_NAME_PREFIX}{sanitized}");
    // Postgres identifiers are silently truncated, not rejected, past 63
    // bytes — truncating here too means two long names that only differ
    // in their tail can never silently collide on the same database.
    name.truncate(63);
    name
}

/// Ensures [`TEMPLATE_DB_NAME`] exists and is migrated to *this build's*
/// exact migration content — not merely "migrated to something" — then
/// drops (if present) and recreates this test binary's own database as a
/// copy of it. The "drop and recreate on every call", not just "create if
/// absent", is what stops rows accumulating across reruns of the same
/// binary. Runs the whole sequence under one held advisory lock so a
/// concurrent process can never observe (or create from) a template that's
/// only half-migrated, and never race this binary's own drop-then-create
/// against another process doing the same for a different binary's
/// database.
///
/// # Why staleness detection keys on migration *content*, not table
/// existence
///
/// An earlier version of this function asked only "does `public.messages`
/// exist in the template?" — true forever after the template's first
/// migration, regardless of what `backends/migrations/postgres/**` looks
/// like by the time a *later* run asks the same question. Because the
/// container (and the `vsms_template` database inside it) is deliberately
/// left running between local `cargo test` invocations (see this module's
/// top-level doc), that made a bootstrap-only schema change — e.g. editing
/// `0002_bootstrap` without touching `0001_init` — silently invisible to
/// every suite until someone happened to run `just test-live-clean` or
/// otherwise blew away the container. That already bit a real PR once: it
/// only passed after a forced clean rebuild. A harness that silently runs
/// tests against a schema older than the one in the working tree is the
/// same class of defect as a CI job that never runs the tests at all — it
/// reports green without having checked anything real.
///
/// The fix: [`migrations_fingerprint`] hashes every `up.sql`'s path and
/// bytes, and that fingerprint is stamped onto the template database
/// itself via `COMMENT ON DATABASE` (a shared, cluster-level catalog
/// entry — `pg_shdescription` — rather than an application table, so it
/// is never copied into any per-binary scratch database by `CREATE
/// DATABASE ... TEMPLATE`, and Postgres's own `dropdb()` cleans it up for
/// free the moment the template is dropped). Every call compares the
/// stamped fingerprint against a freshly computed one; any mismatch —
/// content changed, a migration added or removed, or no stamp at all
/// (first run ever) — triggers a full drop-and-remigrate of the template,
/// exactly as if the template had never existed. A matching fingerprint
/// is the only case that reuses the existing template outright.
async fn ensure_binary_database(base_url: &str) -> String {
    let name = binary_database_name();
    let admin_url = set_database_name(base_url, "postgres");
    let current_fingerprint = migrations_fingerprint();

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connecting to the admin `postgres` database to provision a scratch database");
    let mut conn = pool
        .acquire()
        .await
        .expect("acquiring the single connection this pool holds");

    query("SELECT pg_advisory_lock($1, $2)")
        .bind(MIGRATION_LOCK_NS)
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .expect("acquiring the provisioning advisory lock");

    let template_exists: bool =
        query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(TEMPLATE_DB_NAME)
            .fetch_one(&mut *conn)
            .await
            .expect("checking whether the template database exists");

    // `pg_shdescription` is keyed by (objoid, classoid), joined here
    // against `pg_database` by name; `LEFT JOIN` so a template that exists
    // but was never stamped (shouldn't happen post-fix, but cheap to
    // handle) reads as `None` — a mismatch, not a query error.
    let stored_fingerprint: Option<String> = if template_exists {
        query_scalar(
            "SELECT sd.description FROM pg_database d \
             LEFT JOIN pg_shdescription sd \
               ON sd.objoid = d.oid AND sd.classoid = 'pg_database'::regclass \
             WHERE d.datname = $1",
        )
        .bind(TEMPLATE_DB_NAME)
        .fetch_one(&mut *conn)
        .await
        .expect("reading the template database's stamped migration fingerprint")
    } else {
        None
    };

    let stale = stored_fingerprint.as_deref() != Some(current_fingerprint.as_str());

    // sqlx 0.9.0 (#3723, pulled in transitively by the cratestack 0.11.0
    // bump) narrowed every `query*()`/`raw_sql()` entry point to `impl
    // SqlSafeStr`, implemented only for `&'static str` and the
    // `AssertSqlSafe` wrapper — a runtime `String` no longer satisfies it
    // on its own. Every dynamic string below is genuinely audited, not
    // just silenced: Postgres has no bind-parameter form for an
    // identifier (`CREATE`/`DROP DATABASE "<name>"` can't parameterise the
    // database name), which is the actual reason this harness builds SQL
    // strings at all rather than the R1 exception being casual about it.
    // `TEMPLATE_DB_NAME` is a compile-time constant; `name` is this
    // binary's own crate-name-derived database name (`database_name()`,
    // below); `current_fingerprint` is documented at its own call site as
    // always exactly 16 lowercase hex digits. None crosses a trust
    // boundary this harness doesn't already own.
    if stale {
        if template_exists {
            query(AssertSqlSafe(format!(
                "DROP DATABASE IF EXISTS \"{TEMPLATE_DB_NAME}\" WITH (FORCE)"
            )))
            .execute(&mut *conn)
            .await
            .expect("dropping the stale template database before remigrating it");
        }
        query(AssertSqlSafe(format!(
            "CREATE DATABASE \"{TEMPLATE_DB_NAME}\""
        )))
        .execute(&mut *conn)
        .await
        .expect("creating the template database");

        // Connects to (and fully disconnects from) the template on its
        // own — Postgres refuses `CREATE DATABASE ... TEMPLATE x` while
        // any session is still connected to `x`, so this must return with
        // zero lingering connections before the per-binary `CREATE
        // DATABASE` below runs. The admin connection above stays
        // connected to `postgres`, a different database, so it never
        // counts against that check.
        let template_url = set_database_name(base_url, TEMPLATE_DB_NAME);
        run_psql_migrations(&template_url).await;

        // Escaped by construction, not by string-escaping: `current_fingerprint`
        // is always exactly 16 lowercase hex digits (see
        // `migrations_fingerprint`), so it can never contain a `'` or
        // otherwise break out of the literal.
        query(AssertSqlSafe(format!(
            "COMMENT ON DATABASE \"{TEMPLATE_DB_NAME}\" IS '{current_fingerprint}'"
        )))
        .execute(&mut *conn)
        .await
        .expect("stamping the template database with its migration fingerprint");
    }

    query(AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&mut *conn)
    .await
    .unwrap_or_else(|e| panic!("dropping this binary's own stale database {name}: {e}"));
    query(AssertSqlSafe(format!(
        "CREATE DATABASE \"{name}\" TEMPLATE \"{TEMPLATE_DB_NAME}\""
    )))
    .execute(&mut *conn)
    .await
    .unwrap_or_else(|e| panic!("creating this binary's own database {name} from template: {e}"));

    query("SELECT pg_advisory_unlock($1, $2)")
        .bind(MIGRATION_LOCK_NS)
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .expect("releasing the provisioning advisory lock");

    set_database_name(base_url, &name)
}

/// A deterministic fingerprint of every `up.sql`/`up.pre.sql` this run
/// would apply — both path and byte content, in [`migration_dirs`]'s own
/// lexical order — so renaming, reordering, or editing a migration (or
/// its optional pre-script) all change the result. `up.pre.sql` is
/// conditional (only cratestack migrate diff >=0.11.0 scaffolds one, and
/// only for a blocking migration), folded in immediately before its own
/// `up.sql` when present — matching the order [`run_psql_migrations`]
/// actually applies them in, so a template database can never go stale
/// against an `up.pre.sql` edit the way #87/#102's own fingerprint gap
/// this function already exists to close would otherwise allow. FNV-1a
/// rather than a cryptographic hash: nothing here is security-sensitive,
/// this only needs to detect "did the input change" for a throwaway test
/// database, and FNV-1a needs no extra dependency. Not guaranteed stable
/// across changes to *this function* itself, which is fine — a
/// fingerprint-algorithm change just forces one extra template rebuild on
/// the next run, not a correctness gap.
fn migrations_fingerprint() -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    let mut fold = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    };
    let mut fold_file = |path: &Path| {
        fold(path.to_string_lossy().as_bytes());
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        fold(&bytes);
    };

    for dir in migration_dirs() {
        let up = dir.join("up.sql");
        if !up.exists() {
            continue;
        }
        let up_pre = dir.join("up.pre.sql");
        if up_pre.exists() {
            fold_file(&up_pre);
        }
        fold_file(&up);
    }

    format!("{hash:016x}")
}

/// Shells out to `psql`, the same tool and the same per-file invocation
/// `ci/apply-migrations.sh` already uses — deliberately not reimplemented
/// as a naive `;`-split-and-execute over `sqlx`, which would mis-split the
/// trigger function bodies `0001_init` defines (`CREATE FUNCTION ... AS $$
/// ... ; ... $$`) the moment one contains a semicolon of its own. `psql`
/// already parses that correctly; re-deriving it would be a second, worse
/// SQL parser living in this crate for no reason.
async fn run_psql_migrations(url: &str) {
    // #153's `cratestack_idempotency` bookkeeping table (needed because
    // `IdempotencyLayer` is mounted unconditionally on every route
    // `sms_api::router` builds) used to be a separate step here, applying
    // `ci/idempotency-table.sql` after this loop. It now lives at
    // `backends/migrations/postgres/0003_idempotency_table/up.sql` instead,
    // so this loop alone already covers it.
    for dir in migration_dirs() {
        let up = dir.join("up.sql");
        if !up.exists() {
            continue;
        }
        // `up.pre.sql` (cratestack migrate diff >=0.11.0, scaffolded for a
        // blocking migration) runs first, matching the order
        // `backends/apps/sms-migrate`'s own production runner applies
        // them in — see that binary's own "up.pre.sql" doc section for
        // the full contract. This harness doesn't reproduce that
        // runner's single-transaction guarantee (two separate `psql`
        // invocations, not one), which is fine for a test fixture: every
        // committed `up.pre.sql` is comment-only, and this function's job
        // is getting a schema into a scratch database, not proving
        // production transactional behaviour a second time.
        let up_pre = dir.join("up.pre.sql");
        if up_pre.exists() {
            apply_one_sql_file(url, &up_pre).await;
        }
        apply_one_sql_file(url, &up).await;
    }
}

/// One `psql -f <file>` invocation against `url` — the shared body behind
/// both `up.pre.sql` and `up.sql` in [`run_psql_migrations`], so the two
/// don't duplicate the same `Command` plumbing and drift.
async fn apply_one_sql_file(url: &str, file: &Path) {
    let output = Command::new("psql")
        .arg(url)
        .args(["-v", "ON_ERROR_STOP=1", "-q", "-f"])
        .arg(file)
        .output()
        .await
        .expect("spawning `psql` — is the postgresql-client package installed?");
    assert!(
        output.status.success(),
        "applying {} failed: {}",
        file.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every subdirectory of `backends/migrations/postgres`, sorted lexically —
/// `0001_init` before `0002_bootstrap`, matching `ci/apply-migrations.sh`'s
/// own ordering exactly.
fn migration_dirs() -> Vec<PathBuf> {
    let root = migrations_root();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("reading {}: {e}", root.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// `backends/migrations/postgres`, resolved relative to this crate's own
/// manifest directory rather than the current working directory — this
/// crate is workspace-internal and never published, so a path relative to
/// `CARGO_MANIFEST_DIR` (fixed at compile time, always
/// `<repo>/backends/crates/sms-test-support`) is reliable regardless of which
/// directory `cargo test` happens to be invoked from.
fn migrations_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../migrations/postgres")
        .canonicalize()
        .expect(
            "backends/migrations/postgres must exist two levels up from \
             backends/crates/sms-test-support — has the workspace layout changed?",
        )
}
