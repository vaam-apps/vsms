//! One shared, self-healing Postgres 16 container for every
//! `*_live_postgres.rs` / `*_live.rs` suite in this workspace, replacing the
//! old convention of a human running `docker run ... postgres:16` and
//! `./ci/apply-migrations.sh` by hand before `cargo test -- --ignored`.
//!
//! # Docker Compose, not raw `docker run`
//!
//! This module drives the root `compose.yml`'s `postgres` service via
//! `docker compose`, not a hand-built `docker run` argument list — the
//! maintainer's standing direction ("Compose over raw `docker run`, for
//! full portability") applied to the one place in this workspace that was
//! still shelling out to `docker run` directly. It runs under its own,
//! reserved Compose **project name** ([`COMPOSE_PROJECT`],
//! `vsms-test-harness`) rather than whatever project name a developer's own
//! `docker compose up` against the same file would use (`compose.yml`'s
//! own default, `vsms`) — Compose scopes every container, network, and
//! volume it creates to the project name that created it
//! (`com.docker.compose.project`, a label Compose itself applies and
//! enforces, not a convention this crate has to maintain), so this
//! harness's own teardown ([`ensure_running`]'s sweep, and
//! `just test-live-clean`) structurally cannot reach a container started
//! under a different project — including a developer's own plain
//! `docker compose up` against this exact same file. See this module's
//! "why Compose changes (and doesn't change) each property" section, near
//! the bottom of this doc, for the previous `docker run`-based design's own
//! five load-bearing properties and what happened to each one.
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
//! exactly one container by talking to the `docker compose` CLI directly,
//! keyed by a **fixed project + service name** ([`COMPOSE_PROJECT`] /
//! [`COMPOSE_SERVICE`]), which Compose in turn maps to a **fixed,
//! deterministic container name** (`vsms-test-harness-postgres-1`, observed
//! live — see the naming section below):
//!
//! - The first caller in a session (any process, any test binary) that finds
//!   the service not running starts it via `docker compose up -d --wait` —
//!   self-healing across a crashed or stopped prior run the same way any
//!   `docker compose up` is, since Compose revives rather than errors on an
//!   existing, non-running container with matching config.
//! - Every later caller — the next test in this binary, or the next test
//!   *binary* entirely, since `cargo test --workspace` runs them one OS
//!   process at a time — finds the service already running and reuses it.
//!
//! Net effect: at most one `sms-test-support`-managed container exists at
//! any moment, its count does not grow across repeated `cargo test` runs
//! (the fixed project+service name makes a rerun reuse or revive, never
//! add), and nothing depends on `Drop`, a `static`'s destructor, or an
//! out-of-process reaper to keep that true. The container is deliberately
//! left running after a test run finishes — for the same reason a developer
//! used to leave their manual `docker run` running between local test
//! invocations, so the next run doesn't pay Postgres's startup cost again —
//! but never grows: `just test-live-clean` (`docker compose -p
//! vsms-test-harness -f compose.yml down --volumes --remove-orphans`)
//! removes it, its network, and its named volume when it is genuinely no
//! longer wanted.
//!
//! # Concurrent processes racing to create the container
//!
//! `cargo test --workspace` does not trigger this — binaries run
//! sequentially. If a developer instead runs two separate
//! `cargo test -p <crate> --test <name> -- --ignored` invocations
//! concurrently, both may observe "not running" and both attempt
//! `docker compose up -d --wait`. This is genuinely simpler than the old
//! `docker run --name` design, not just differently shaped: `docker run
//! --name` fails outright with a distinguishable `"Conflict"` on a losing
//! race, which the old code had to detect by matching that string in
//! `stderr` and fall back to reuse; `docker compose up` is designed to be
//! idempotent against its own project state, so there is no equivalent
//! string to match on a losing race — [`ensure_running`] instead just
//! re-checks whether the service ended up running after a failed `up` and
//! only treats it as a real failure if it didn't. The narrower race — two
//! processes each deciding at nearly the same instant that the *old,
//! pre-Compose* container (see the migration note below) needs removing —
//! is the same accepted-churn shape the previous design already lived with,
//! scoped even more narrowly now (an exact container name, not a label
//! filter matching however many containers carry it).
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
//! # Migrating from the old `docker run` design, and the five properties it
//! # had to preserve
//!
//! A machine that last ran the pre-Compose version of this module may still
//! have a container named [`LEGACY_CONTAINER_NAME`] — that design's own
//! fixed `docker run --name` — sitting on [`HOST_PORT`], which would make
//! Compose's own `up` fail to bind that port for its differently-named
//! container. [`ensure_running`] removes it first, by its exact, literal
//! name only, before ever calling `docker compose up` — a one-time
//! migration step, not a recurring sweep, and it can never reach anything
//! this crate did not itself create under the old design, the same
//! guarantee the old label-based sweep gave for a different reason (see
//! below).
//!
//! The task this conversion was built against named five properties the old
//! design had each learned by something breaking, and required each to
//! either survive intact or have its replacement justified in the open.
//! Restated here, against what actually changed:
//!
//! 1. **Nothing may depend on a destructor running at process exit.**
//!    Unchanged — this module still never puts a container handle in a
//!    `static`, Compose or not; see above.
//! 2. **Cleanup must be scoped so it can never touch an unrelated
//!    container.** Changed, and arguably strengthened: the old design's
//!    custom label (`dev.vsms.test-harness=true`) is gone. In its place, every cleanup path
//!    ([`ensure_running`]'s own sweep, and `just test-live-clean`) scopes by
//!    [`COMPOSE_PROJECT`] — a name Docker Compose itself stamps onto every
//!    resource it creates and that every `docker compose down`/`ps`
//!    invocation is scoped by construction, not by a filter this crate
//!    chooses to apply consistently. A hand-applied label can be (and, if
//!    this workspace's history is any guide, eventually would be) copied
//!    onto an unrelated container by a future edit that doesn't understand
//!    why it's there; a Compose project name cannot be attached to a
//!    container this crate didn't create except by another process
//!    deliberately reusing the same reserved name. Proven live, not just
//!    argued: a genuinely unrelated `postgres:16-alpine` container was
//!    started by hand under an unrelated name, this crate's own
//!    `docker compose -p vsms-test-harness -f compose.yml down --volumes
//!    --remove-orphans` was run, and the unrelated container was confirmed
//!    still running afterward — see the PR description for the captured
//!    output. **Never `docker prune` in any form, under any flag** — that
//!    rule doesn't change just because the scoping mechanism did.
//! 3. **One database per test binary, not one shared database.** Unchanged
//!    — this property lives entirely in [`ensure_binary_database`], below,
//!    which knows nothing about how the container it connects to was
//!    started.
//! 4. **The migration check fingerprints migration content, not merely
//!    whether a table exists.** Unchanged, same reason as above — this is
//!    [`migrations_fingerprint`]'s job, unrelated to container mechanics.
//! 5. **The container name is fixed and global, so two concurrent `cargo
//!    test` invocations on one machine corrupt each other's runs.** Still
//!    true, and deliberately not fixed here: Compose derives the actual
//!    container name (`vsms-test-harness-postgres-1`, observed live) from
//!    [`COMPOSE_PROJECT`] + [`COMPOSE_SERVICE`], but that derivation is
//!    itself fixed and global — Docker container names are unique
//!    cluster-wide regardless of which Compose project "owns" one, so two
//!    concurrent invocations still race over the same name the same way
//!    they did before. Compose's own idempotency (see "Concurrent processes
//!    racing" above) makes the failure mode *milder* — a losing `docker run
//!    --name` used to hard-fail with `"Conflict"`, where a losing `docker
//!    compose up` is more often harmless — but it does not
//!    make concurrent invocations *safe* in the sense of each getting its
//!    own isolated Postgres. Scoping the project name per-invocation (e.g.
//!    a PID or random suffix) would fix this properly and was deliberately
//!    not done: it's a bigger change than this conversion's own scope, and
//!    the limitation is exactly as documented as it was before, not newly
//!    introduced.
//!
//! One more thing found live while building this, not anticipated going
//! in: `compose.yml`'s port-mapping shorthand (`"<port>:5432"`, no host
//! part) binds every interface, not loopback-only — confirmed by running
//! it and seeing `0.0.0.0:55432->5432/tcp` in `docker ps`. The old `docker
//! run -p 127.0.0.1:{HOST_PORT}:5432` was loopback-only on purpose (this
//! module's own doc on [`HOST_PORT`] says so), so `compose.yml`'s mapping
//! is `"127.0.0.1:${VSMS_POSTGRES_PORT:-5432}:5432"` explicitly, not the
//! shorthand, with a comment at the call site recording why.
//!
//! Also worth naming: `docker compose up -d` is idempotent in a way `docker
//! run --name` never was, which simplified the sweep-then-create structure
//! down to a single call — see "Concurrent processes racing" above for the
//! detail. And the named volume (`vsms_pgdata`, `compose.yml`) is new:
//! the old design never created one (Postgres's data lived in the
//! container's own writable layer, gone the instant `docker rm -f` ran), so
//! `just test-live-clean` now passes `--volumes` to actually match that
//! same "fully reset" behaviour — a plain `docker compose down` without it
//! would remove the container but silently leave the volume (and every row
//! in it) behind as orphaned state the old design never had to worry about.
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
//! `backends/apps/sms-worker/tests/kill9_reclaim_live.rs` cannot: it spawns a *real*
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
//! working tree's `backends/migrations/postgres/**` no longer matches the
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
//! This crate is a named R1 exception in its own right — `CONTRIBUTING.md`'s
//! own "Live-Postgres test harness" row — and `cargo xtask no-raw-sqlx`
//! allowlists this file by path. It fits the *spirit* of the "migrations"
//! exception too: every raw query below exists only to decide whether
//! `backends/migrations/postgres/**` has already been applied to a throwaway
//! test database, and to serialize that decision (via
//! `pg_advisory_lock`/`pg_advisory_unlock` — itself the *second* named
//! exception) against every other process racing to ask the same question.
//! No application row is ever read or written here.
//!
//! # The `rustls` crypto provider gap the binaries' own fix didn't cover
//!
//! `backends/apps/sms-gateway/src/main.rs` and `backends/apps/sms-worker/src/main.rs` each call
//! `rustls::crypto::ring::default_provider().install_default()` as the
//! first line of `main`, documented at length on that call site: dropping
//! `aws-lc-rs` (`AGENTS.md`'s "rustls, musl, and distroless" section) left
//! authkestra's own `reqwest` 0.13.4 dependency — pinned
//! `default-features = false, features = ["rustls-no-provider"]` — with no
//! crypto backend baked in, so the *first* TLS handshake it attempts
//! panics unless something has already installed a provider process-wide.
//! That fix covers every production binary, because `main` always runs.
//! **It does not cover test binaries**, whose entry point is the libtest
//! harness, not either crate's `main` — so any live suite that exercises a
//! code path building a `reqwest` 0.13.4 client (this workspace's own
//! `GatewayAuth`, via `authkestra_resource::jwt`'s `JwksCache`, is the one
//! found live: it fetches the issuer's JWKS over HTTP to validate a real
//! token, in-process, in whichever suite constructs it) panics with "No
//! rustls crypto provider is configured" the same way an unfixed binary
//! would have, and did — `backends/crates/sms-auth/tests/oidc_flow_live.rs` and
//! `backends/crates/sms-auth/tests/rbac_layer2_live_postgres.rs` both construct
//! `GatewayAuth` in-process and both panicked this way before
//! [`install_default_crypto_provider`] existed. Every other live suite in
//! this workspace was checked (`cargo test --workspace --no-fail-fast --
//! --ignored`, all 19 live test binaries, not just the one CI happened to
//! stop on — `cargo test` is fail-fast across a workspace by default,
//! which is why a single red binary hid the rest) and does not: the
//! subprocess-spawning suites (`backends/apps/sms-gateway/tests/
//! m1_acceptance_gate_live_postgres.rs`, `.../provision_client_cli_live_
//! postgres.rs`, `backends/apps/sms-worker/tests/kill9_reclaim_live.rs`) exercise
//! `GatewayAuth` only inside a real `sms-gateway`/`sms-worker` child
//! process, whose own `main` already installs the provider;
//! `backends/crates/sms-auth/tests/provision_app_client_live_postgres.rs` builds an
//! in-process OP router but never a `GatewayAuth`, so it never reaches
//! `authkestra_resource::jwt` at all.
//!
//! [`install_default_crypto_provider`] is called unconditionally at the
//! top of [`database_url`] rather than left for each live suite to
//! remember: every live suite in this workspace already calls
//! `database_url` (it is how each one gets a migrated Postgres — see this
//! module's own doc), so hooking the install there is the one change that
//! reaches all of them, present and future, with no per-test-file edit and
//! nothing to re-discover the next time a suite starts exercising
//! `GatewayAuth`. `backends/apps/sms-gateway/tests/tls_no_provider_live.rs` is the
//! one live suite that needs no database and so never calls
//! `database_url` — it already installs the provider itself, for the same
//! reason its own module doc gives (it exists specifically to prove this
//! exact ordering, independently of this crate).

use std::path::{Path, PathBuf};
use std::time::Duration;

use cratestack::sqlx::postgres::PgPoolOptions;
use cratestack::sqlx::{query, query_scalar};
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
