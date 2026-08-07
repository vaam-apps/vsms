//! One shared, self-healing Postgres 16 container for every
//! `*_live_postgres.rs` / `*_live.rs` suite in this workspace, replacing the
//! old convention of a human running `docker run ... postgres:16` and
//! `./ci/apply-migrations.sh` by hand before `cargo test -- --ignored`.
//!
//! # Why not `testcontainers`' usual `ContainerAsync` + `static` pattern
//!
//! An earlier attempt at this harness cached a `testcontainers::ContainerAsync`
//! in a process-level `static` (`OnceCell`), on the reasoning that starting
//! Postgres once per test *binary* (`cargo test` runs binaries sequentially,
//! not concurrently — confirmed by `ca653a1`/#102's own investigation, see
//! below) was cheap enough. It leaked every container it ever started: Rust
//! never runs `Drop` for a `static`'s contents on any process-exit path, so
//! `ContainerAsync::drop`'s stop-and-remove logic — the *only* thing that
//! would have cleaned it up — simply never ran. 14 test binaries, each
//! starting its own uniquely-named container that outlived the process,
//! accumulate without bound across repeated runs. This workspace has no
//! Ryuk reaper sidecar installed to catch what `Drop` missed either.
//!
//! This module never puts a container handle in a `static` at all, and never
//! relies on any Rust destructor running at process exit. Instead it manages
//! exactly one container by talking to the `docker` CLI directly, keyed by a
//! **fixed, deterministic name** ([`CONTAINER_NAME`]) carrying a **distinctive
//! label** ([`LABEL`]):
//!
//! - The first caller in a session (any process, any test binary) that finds
//!   no container with that name running starts one, after first
//!   force-removing anything already carrying [`LABEL`] — self-healing
//!   across a crashed or stale prior run, and a hard guarantee that a bulk
//!   sweep can never touch a container it did not create, no matter what
//!   else is running on the machine.
//! - Every later caller — the next test in this binary, or the next test
//!   *binary* entirely, since `cargo test --workspace` runs them one OS
//!   process at a time — finds the fixed name already running and reuses it.
//!
//! Net effect: at most one `sms-test-support`-managed container exists at
//! any moment, its count does not grow across repeated `cargo test` runs
//! (the fixed name makes a rerun replace, or simply reuse, rather than add),
//! and nothing depends on `Drop`, a `static`'s destructor, or an
//! out-of-process reaper to keep that true. The container is deliberately
//! left running after a test run finishes — for the same reason a developer
//! used to leave their manual `docker run` running between local test
//! invocations, so the next run doesn't pay Postgres's startup cost again —
//! but never grows: `just test-live-clean` (or a plain
//! `docker rm -f vsms-test-harness-postgres`) removes it by name when it is
//! genuinely no longer wanted.
//!
//! # Concurrent processes racing to create the container
//!
//! `cargo test --workspace` does not trigger this — binaries run
//! sequentially. If a developer instead runs two separate
//! `cargo test -p <crate> --test <name> -- --ignored` invocations
//! concurrently, both may observe "not running" and both attempt to create
//! it; `docker run --name` makes container-name reservation atomic, so at
//! most one `docker run` actually succeeds and the loser falls back to
//! reusing what the winner created (see [`ensure_running`]). The narrower
//! race — both processes independently deciding to *sweep* at nearly the
//! same instant, with one sweeping away a container the other just created
//! — is a known, accepted limitation of this simpler design: it can
//! produce wasted container churn under genuinely concurrent invocations,
//! but never an unbounded leak, since a sweep only ever removes containers
//! carrying [`LABEL`] and a fresh one is always created right behind it.
//!
//! # What this does *not* solve, on purpose
//!
//! `ca653a1` (#102) found that this workspace's live suites need a second,
//! unrelated kind of serialization: tests **within one binary** run
//! concurrently by Rust's default multi-threaded test harness and can race
//! each other over shared candidate rows, or over Postgres's own `pg_type`
//! catalog on a truly cold cache. Its fix — a per-binary
//! `tokio::sync::Mutex` every test acquires for its whole body — addresses a
//! problem this module cannot: it is intra-binary contention on the schema
//! and seeded rows, not inter-process contention on which Postgres exists.
//! Nothing here changes or removes those mutexes; they stay exactly as
//! `ca653a1` left them — tests within one binary still share that binary's
//! own database (see the next section), so the same race `ca653a1` fixed is
//! still possible within it.
//!
//! # One database per test binary
//!
//! The container above solves inter-process container accumulation, but it
//! does not by itself solve inter-*binary* row contention: early versions of
//! this harness pointed every one of the 14 test binaries at the same
//! `vsms` database inside the one shared container. That database persists
//! across runs by design (see above), so it accumulates rows from every
//! suite that has ever run against it. Most suites tolerate this — they
//! scope their own queries by a freshly generated app/message id — but
//! `app/sms-worker/tests/kill9_reclaim_live.rs` cannot: it spawns a *real*
//! `sms-worker --roles dispatch` process, and that process's claim loop
//! deliberately selects **any** eligible candidate row, the same way it
//! would in production. Run after the other 13 suites have left claimable
//! `accepted`/`queued`/`routed` rows behind, it picks up their leftovers
//! instead of the one row this test seeded and cares about, and the test's
//! own message never advances — a real bug, found by running the full
//! `just test-live` sweep rather than any single suite in isolation.
//!
//! The fix: every test binary gets its own database inside the one shared
//! container, derived deterministically from the test binary's own
//! executable name ([`binary_database_name`] — see its own doc for a
//! second real collision this design found and fixed live, between two
//! *different* test binaries that happen to share a file name) so reruns
//! of the same binary reuse (after dropping and recreating — see below)
//! rather than endlessly accumulate databases. [`ensure_binary_database`]
//! drops and recreates it from a shared template database (`vsms_template`)
//! on every call — "drop and recreate", not just "create if absent", is
//! what actually stops row accumulation across reruns, not merely the
//! per-binary split. Applying both migration files via `psql` 14 times
//! (once per binary) measured slower than applying them once to a template
//! and using Postgres's own `CREATE DATABASE ... TEMPLATE ...`, which is a
//! filesystem-level copy — so every binary's own database creation is just
//! that copy plus a `DROP DATABASE ... WITH (FORCE)` first. `WITH (FORCE)`
//! (Postgres 13+) terminates any lingering connections to the target
//! database itself, which matters for exactly this test: a prior run's
//! `kill -9`'d `sms-worker` subprocess could in principle leave a dangling
//! connection open to its own scratch database.
//!
//! The template is re-migrated whenever [`migrations_fingerprint`] of the
//! working tree's `schema/migrations/postgres/**` no longer matches the
//! fingerprint stamped onto the template the last time it was built (via
//! `COMMENT ON DATABASE`, checked against `pg_shdescription`) — not merely
//! "at most once per container lifetime" the way an earlier version of
//! this module worked. That earlier version asked only "does
//! `public.messages` exist in the template?", which stayed true forever
//! after the template's first-ever migration regardless of what the
//! migration files looked like on any later run — since the container (and
//! the template inside it) is deliberately left running between local
//! `cargo test` invocations, that let a bootstrap-only schema edit run
//! silently against a stale template until something forced a full
//! `just test-live-clean`. See [`ensure_binary_database`]'s own doc for the
//! fingerprint mechanism.
//!
//! Postgres refuses to `CREATE DATABASE ... TEMPLATE x` while any other
//! session is connected to `x`, so remigrating the template always shells
//! out to `psql` (its own separate, self-closing connections) rather than
//! reusing [`ensure_binary_database`]'s own admin connection to run DDL
//! against it directly, and the whole ensure-template / drop-and-recreate-
//! binary-db sequence runs under one held advisory lock (reusing
//! [`MIGRATION_LOCK_NS`]/[`MIGRATION_LOCK_KEY`]) so two processes can't
//! interleave a template rebuild with another's `CREATE DATABASE ...
//! TEMPLATE`. `kill9_reclaim_live.rs` needs no change for this: it already
//! reads its database URL once from [`database_url`] and passes that exact
//! string to the `sms-worker` subprocess it spawns via `--database-url`, so
//! both the test and the process it kills automatically share the same
//! per-binary scratch database without either needing to know it's
//! per-binary at all.
//!
//! # R1
//!
//! This crate is a fourth path calling raw `sqlx`, alongside the three
//! named in `CONTRIBUTING.md` (migrations, advisory locks, `LISTEN`/
//! `NOTIFY`) — `ci/assert-no-raw-sqlx.sh` allowlists this file by path. It
//! fits the *spirit* of the "migrations" exception rather than being a new
//! category: every raw query below exists only to decide whether
//! `schema/migrations/postgres/**` has already been applied to a throwaway
//! test database, and to serialize that decision (via
//! `pg_advisory_lock`/`pg_advisory_unlock` — itself the *second* named
//! exception) against every other process racing to ask the same question.
//! No application row is ever read or written here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::sqlx::{query, query_scalar};
use tokio::process::Command;
use tokio::sync::OnceCell;

/// Fixed, deterministic name — the whole self-healing/no-accumulation
/// strategy in this module's own doc depends on every process agreeing on
/// this one name rather than generating a fresh one per run.
const CONTAINER_NAME: &str = "vsms-test-harness-postgres";

/// Label every container this module ever creates carries, and the *only*
/// thing any cleanup here is ever scoped by. Never remove a container by
/// image name or a bare name pattern — see the top-level task brief this
/// crate was built against: a bare-name or image-based sweep on a
/// developer's machine can destroy containers this workspace did not
/// create.
const LABEL_KEY: &str = "dev.vsms.test-harness";
const LABEL: &str = "dev.vsms.test-harness=true";

/// Bound to loopback only — never `0.0.0.0` — so this is never reachable
/// from outside the machine running the tests. Deliberately not `5432`:
/// this workspace's own module docs (e.g. `crates/sms-worker/tests/
/// live_postgres.rs`) tell a human to run their own `docker run -p 5432:5432
/// postgres:16` for ad hoc use, and this harness must never collide with
/// that or with a developer's own local Postgres.
const HOST_PORT: u16 = 55432;

const IMAGE: &str = "postgres:16";

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
    DATABASE_URL
        .get_or_init(|| async { ensure_postgres().await })
        .await
        .clone()
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
    format!("postgres://postgres:postgres@127.0.0.1:{HOST_PORT}/vsms")
}

/// Makes sure [`CONTAINER_NAME`] exists and is running, starting a fresh one
/// (after sweeping any stale, [`LABEL`]-carrying leftovers) if not.
async fn ensure_running() {
    if container_running().await {
        return;
    }

    sweep_labelled_containers().await;

    let output = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            CONTAINER_NAME,
            "--label",
            LABEL,
            "-p",
            &format!("127.0.0.1:{HOST_PORT}:5432"),
            "-e",
            "POSTGRES_PASSWORD=postgres",
            "-e",
            "POSTGRES_DB=vsms",
            IMAGE,
        ])
        .output()
        .await
        .expect("spawning `docker run` — is Docker installed and on PATH?");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Another process running the exact same `cargo test --workspace`
        // sweep won the race to create the container between our own
        // `container_running` check and this `docker run` — not our
        // problem to fix, just to notice and fall back to reusing what it
        // made. Anything else is a real failure.
        assert!(
            stderr.contains("Conflict") || stderr.contains("already in use"),
            "docker run failed to start {IMAGE} as {CONTAINER_NAME}: {stderr}"
        );
    }
}

async fn container_running() -> bool {
    let output = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", CONTAINER_NAME])
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() == "true",
        _ => false,
    }
}

/// Force-removes every container carrying [`LABEL`] — and only those.
/// Scoped by label, never by image or a bare name pattern: this is the one
/// piece of this module that must never be loosened, since a sweep filtered
/// by `ancestor=postgres:16` or a name substring would just as happily
/// remove a developer's own, completely unrelated Postgres container.
async fn sweep_labelled_containers() {
    let list = Command::new("docker")
        .args(["ps", "-aq", "--filter", &format!("label={LABEL_KEY}")])
        .output()
        .await
        .expect("spawning `docker ps` to sweep stale test-harness containers");
    let ids: Vec<&str> = std::str::from_utf8(&list.stdout)
        .expect("docker ps output is valid UTF-8")
        .split_whitespace()
        .collect();
    if ids.is_empty() {
        return;
    }
    let mut cmd = Command::new("docker");
    cmd.arg("rm").arg("-f").args(&ids);
    // Best-effort: if a listed container vanished between `ps` and `rm`
    // (e.g. another process's sweep beat us to it), that is exactly the
    // outcome we wanted anyway.
    let _ = cmd.output().await;
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
                    "Postgres in {CONTAINER_NAME} never became reachable within 30s: {source}"
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
/// code review: `crates/sms-auth/tests/live_postgres.rs` and
/// `crates/sms-worker/tests/live_postgres.rs` are two genuinely different
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
/// migration, regardless of what `schema/migrations/postgres/**` looks
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

    if stale {
        if template_exists {
            query(&format!(
                "DROP DATABASE IF EXISTS \"{TEMPLATE_DB_NAME}\" WITH (FORCE)"
            ))
            .execute(&mut *conn)
            .await
            .expect("dropping the stale template database before remigrating it");
        }
        query(&format!("CREATE DATABASE \"{TEMPLATE_DB_NAME}\""))
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
        query(&format!(
            "COMMENT ON DATABASE \"{TEMPLATE_DB_NAME}\" IS '{current_fingerprint}'"
        ))
        .execute(&mut *conn)
        .await
        .expect("stamping the template database with its migration fingerprint");
    }

    query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut *conn)
        .await
        .unwrap_or_else(|e| panic!("dropping this binary's own stale database {name}: {e}"));
    query(&format!(
        "CREATE DATABASE \"{name}\" TEMPLATE \"{TEMPLATE_DB_NAME}\""
    ))
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

/// A deterministic fingerprint of every `up.sql` this run would apply —
/// both path and byte content, in [`migration_dirs`]'s own lexical order —
/// so renaming, reordering, or editing a migration all change the result.
/// FNV-1a rather than a cryptographic hash: nothing here is
/// security-sensitive, this only needs to detect "did the input change"
/// for a throwaway test database, and FNV-1a needs no extra dependency.
/// Not guaranteed stable across changes to *this function* itself, which
/// is fine — a fingerprint-algorithm change just forces one extra
/// template rebuild on the next run, not a correctness gap.
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

    for dir in migration_dirs() {
        let up = dir.join("up.sql");
        if !up.exists() {
            continue;
        }
        fold(up.to_string_lossy().as_bytes());
        let bytes = std::fs::read(&up).unwrap_or_else(|e| panic!("reading {}: {e}", up.display()));
        fold(&bytes);
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
    for dir in migration_dirs() {
        let up = dir.join("up.sql");
        if !up.exists() {
            continue;
        }
        let output = Command::new("psql")
            .arg(url)
            .args(["-v", "ON_ERROR_STOP=1", "-q", "-f"])
            .arg(&up)
            .output()
            .await
            .expect("spawning `psql` — is the postgresql-client package installed?");
        assert!(
            output.status.success(),
            "applying {} failed: {}",
            up.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Every subdirectory of `schema/migrations/postgres`, sorted lexically —
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

/// `schema/migrations/postgres`, resolved relative to this crate's own
/// manifest directory rather than the current working directory — this
/// crate is workspace-internal and never published, so a path relative to
/// `CARGO_MANIFEST_DIR` (fixed at compile time, always
/// `<repo>/crates/sms-test-support`) is reliable regardless of which
/// directory `cargo test` happens to be invoked from.
fn migrations_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/migrations/postgres")
        .canonicalize()
        .expect(
            "schema/migrations/postgres must exist two levels up from \
             crates/sms-test-support — has the workspace layout changed?",
        )
}
