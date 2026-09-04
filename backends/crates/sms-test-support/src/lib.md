One shared, self-healing Postgres 16 container for every
`*_live_postgres.rs` / `*_live.rs` suite in this workspace, replacing the
old convention of a human running `docker run ... postgres:16` and
`./ci/apply-migrations.sh` by hand before `cargo test -- --ignored`.

# Docker Compose, not raw `docker run`

This module drives the root `compose.yml`'s `postgres` service via
`docker compose`, not a hand-built `docker run` argument list — the
maintainer's standing direction ("Compose over raw `docker run`, for
full portability") applied to the one place in this workspace that was
still shelling out to `docker run` directly. It runs under its own,
reserved Compose **project name** ([`COMPOSE_PROJECT`],
`vsms-test-harness`) rather than whatever project name a developer's own
`docker compose up` against the same file would use (`compose.yml`'s
own default, `vsms`) — Compose scopes every container, network, and
volume it creates to the project name that created it
(`com.docker.compose.project`, a label Compose itself applies and
enforces, not a convention this crate has to maintain), so this
harness's own teardown ([`ensure_running`]'s sweep, and
`just test-live-clean`) structurally cannot reach a container started
under a different project — including a developer's own plain
`docker compose up` against this exact same file. See this module's
"why Compose changes (and doesn't change) each property" section, near
the bottom of this doc, for the previous `docker run`-based design's own
five load-bearing properties and what happened to each one.

# Why not `testcontainers`' usual `ContainerAsync` + `static` pattern

An earlier attempt at this harness cached a `testcontainers::ContainerAsync`
in a process-level `static` (`OnceCell`), on the reasoning that starting
Postgres once per test *binary* (`cargo test` runs binaries sequentially,
not concurrently — confirmed by `ca653a1`/#102's own investigation, see
below) was cheap enough. It leaked every container it ever started: Rust
never runs `Drop` for a `static`'s contents on any process-exit path, so
`ContainerAsync::drop`'s stop-and-remove logic — the *only* thing that
would have cleaned it up — simply never ran. 14 test binaries, each
starting its own uniquely-named container that outlived the process,
accumulate without bound across repeated runs. This workspace has no
Ryuk reaper sidecar installed to catch what `Drop` missed either.

This module never puts a container handle in a `static` at all, and never
relies on any Rust destructor running at process exit. Instead it manages
exactly one container by talking to the `docker compose` CLI directly,
keyed by a **fixed project + service name** ([`COMPOSE_PROJECT`] /
[`COMPOSE_SERVICE`]), which Compose in turn maps to a **fixed,
deterministic container name** (`vsms-test-harness-postgres-1`, observed
live — see the naming section below):

- The first caller in a session (any process, any test binary) that finds
  the service not running starts it via `docker compose up -d --wait` —
  self-healing across a crashed or stopped prior run the same way any
  `docker compose up` is, since Compose revives rather than errors on an
  existing, non-running container with matching config.
- Every later caller — the next test in this binary, or the next test
  *binary* entirely, since `cargo test --workspace` runs them one OS
  process at a time — finds the service already running and reuses it.

Net effect: at most one `sms-test-support`-managed container exists at
any moment, its count does not grow across repeated `cargo test` runs
(the fixed project+service name makes a rerun reuse or revive, never
add), and nothing depends on `Drop`, a `static`'s destructor, or an
out-of-process reaper to keep that true. The container is deliberately
left running after a test run finishes — for the same reason a developer
used to leave their manual `docker run` running between local test
invocations, so the next run doesn't pay Postgres's startup cost again —
but never grows: `just test-live-clean` (`docker compose -p
vsms-test-harness -f compose.yml down --volumes --remove-orphans`)
removes it, its network, and its named volume when it is genuinely no
longer wanted.

# Concurrent processes racing to create the container

`cargo test --workspace` does not trigger this — binaries run
sequentially. If a developer instead runs two separate
`cargo test -p <crate> --test <name> -- --ignored` invocations
concurrently, both may observe "not running" and both attempt
`docker compose up -d --wait`. This is genuinely simpler than the old
`docker run --name` design, not just differently shaped: `docker run
--name` fails outright with a distinguishable `"Conflict"` on a losing
race, which the old code had to detect by matching that string in
`stderr` and fall back to reuse; `docker compose up` is designed to be
idempotent against its own project state, so there is no equivalent
string to match on a losing race — [`ensure_running`] instead just
re-checks whether the service ended up running after a failed `up` and
only treats it as a real failure if it didn't. The narrower race — two
processes each deciding at nearly the same instant that the *old,
pre-Compose* container (see the migration note below) needs removing —
is the same accepted-churn shape the previous design already lived with,
scoped even more narrowly now (an exact container name, not a label
filter matching however many containers carry it).

# What this does *not* solve, on purpose

`ca653a1` (#102) found that this workspace's live suites need a second,
unrelated kind of serialization: tests **within one binary** run
concurrently by Rust's default multi-threaded test harness and can race
each other over shared candidate rows, or over Postgres's own `pg_type`
catalog on a truly cold cache. Its fix — a per-binary
`tokio::sync::Mutex` every test acquires for its whole body — addresses a
problem this module cannot: it is intra-binary contention on the schema
and seeded rows, not inter-process contention on which Postgres exists.
Nothing here changes or removes those mutexes; they stay exactly as
`ca653a1` left them — tests within one binary still share that binary's
own database (see the next section), so the same race `ca653a1` fixed is
still possible within it.

# Migrating from the old `docker run` design, and the five properties it
# had to preserve

A machine that last ran the pre-Compose version of this module may still
have a container named [`LEGACY_CONTAINER_NAME`] — that design's own
fixed `docker run --name` — sitting on [`HOST_PORT`], which would make
Compose's own `up` fail to bind that port for its differently-named
container. [`ensure_running`] removes it first, by its exact, literal
name only, before ever calling `docker compose up` — a one-time
migration step, not a recurring sweep, and it can never reach anything
this crate did not itself create under the old design, the same
guarantee the old label-based sweep gave for a different reason (see
below).

The task this conversion was built against named five properties the old
design had each learned by something breaking, and required each to
either survive intact or have its replacement justified in the open.
Restated here, against what actually changed:

1. **Nothing may depend on a destructor running at process exit.**
   Unchanged — this module still never puts a container handle in a
   `static`, Compose or not; see above.
2. **Cleanup must be scoped so it can never touch an unrelated
   container.** Changed, and arguably strengthened: the old design's
   custom label (`dev.vsms.test-harness=true`) is gone. In its place, every cleanup path
   ([`ensure_running`]'s own sweep, and `just test-live-clean`) scopes by
   [`COMPOSE_PROJECT`] — a name Docker Compose itself stamps onto every
   resource it creates and that every `docker compose down`/`ps`
   invocation is scoped by construction, not by a filter this crate
   chooses to apply consistently. A hand-applied label can be (and, if
   this workspace's history is any guide, eventually would be) copied
   onto an unrelated container by a future edit that doesn't understand
   why it's there; a Compose project name cannot be attached to a
   container this crate didn't create except by another process
   deliberately reusing the same reserved name. Proven live, not just
   argued: a genuinely unrelated `postgres:16-alpine` container was
   started by hand under an unrelated name, this crate's own
   `docker compose -p vsms-test-harness -f compose.yml down --volumes
   --remove-orphans` was run, and the unrelated container was confirmed
   still running afterward — see the PR description for the captured
   output. **Never `docker prune` in any form, under any flag** — that
   rule doesn't change just because the scoping mechanism did.
3. **One database per test binary, not one shared database.** Unchanged
   — this property lives entirely in [`ensure_binary_database`], below,
   which knows nothing about how the container it connects to was
   started.
4. **The migration check fingerprints migration content, not merely
   whether a table exists.** Unchanged, same reason as above — this is
   [`migrations_fingerprint`]'s job, unrelated to container mechanics.
5. **The container name is fixed and global, so two concurrent `cargo
   test` invocations on one machine corrupt each other's runs.** Still
   true, and deliberately not fixed here: Compose derives the actual
   container name (`vsms-test-harness-postgres-1`, observed live) from
   [`COMPOSE_PROJECT`] + [`COMPOSE_SERVICE`], but that derivation is
   itself fixed and global — Docker container names are unique
   cluster-wide regardless of which Compose project "owns" one, so two
   concurrent invocations still race over the same name the same way
   they did before. Compose's own idempotency (see "Concurrent processes
   racing" above) makes the failure mode *milder* — a losing `docker run
   --name` used to hard-fail with `"Conflict"`, where a losing `docker
   compose up` is more often harmless — but it does not
   make concurrent invocations *safe* in the sense of each getting its
   own isolated Postgres. Scoping the project name per-invocation (e.g.
   a PID or random suffix) would fix this properly and was deliberately
   not done: it's a bigger change than this conversion's own scope, and
   the limitation is exactly as documented as it was before, not newly
   introduced.

One more thing found live while building this, not anticipated going
in: `compose.yml`'s port-mapping shorthand (`"<port>:5432"`, no host
part) binds every interface, not loopback-only — confirmed by running
it and seeing `0.0.0.0:55432->5432/tcp` in `docker ps`. The old `docker
run -p 127.0.0.1:{HOST_PORT}:5432` was loopback-only on purpose (this
module's own doc on [`HOST_PORT`] says so), so `compose.yml`'s mapping
is `"127.0.0.1:${VSMS_POSTGRES_PORT:-5432}:5432"` explicitly, not the
shorthand, with a comment at the call site recording why.

Also worth naming: `docker compose up -d` is idempotent in a way `docker
run --name` never was, which simplified the sweep-then-create structure
down to a single call — see "Concurrent processes racing" above for the
detail. And the named volume (`vsms_pgdata`, `compose.yml`) is new:
the old design never created one (Postgres's data lived in the
container's own writable layer, gone the instant `docker rm -f` ran), so
`just test-live-clean` now passes `--volumes` to actually match that
same "fully reset" behaviour — a plain `docker compose down` without it
would remove the container but silently leave the volume (and every row
in it) behind as orphaned state the old design never had to worry about.

# One database per test binary

The container above solves inter-process container accumulation, but it
does not by itself solve inter-*binary* row contention: early versions of
this harness pointed every one of the 14 test binaries at the same
`vsms` database inside the one shared container. That database persists
across runs by design (see above), so it accumulates rows from every
suite that has ever run against it. Most suites tolerate this — they
scope their own queries by a freshly generated app/message id — but
`backends/apps/sms-worker/tests/kill9_reclaim_live.rs` cannot: it spawns a *real*
`sms-worker --roles dispatch` process, and that process's claim loop
deliberately selects **any** eligible candidate row, the same way it
would in production. Run after the other 13 suites have left claimable
`accepted`/`queued`/`routed` rows behind, it picks up their leftovers
instead of the one row this test seeded and cares about, and the test's
own message never advances — a real bug, found by running the full
`just test-live` sweep rather than any single suite in isolation.

The fix: every test binary gets its own database inside the one shared
container, derived deterministically from the test binary's own
executable name ([`binary_database_name`] — see its own doc for a
second real collision this design found and fixed live, between two
*different* test binaries that happen to share a file name) so reruns
of the same binary reuse (after dropping and recreating — see below)
rather than endlessly accumulate databases. [`ensure_binary_database`]
drops and recreates it from a shared template database (`vsms_template`)
on every call — "drop and recreate", not just "create if absent", is
what actually stops row accumulation across reruns, not merely the
per-binary split. Applying both migration files via `psql` 14 times
(once per binary) measured slower than applying them once to a template
and using Postgres's own `CREATE DATABASE ... TEMPLATE ...`, which is a
filesystem-level copy — so every binary's own database creation is just
that copy plus a `DROP DATABASE ... WITH (FORCE)` first. `WITH (FORCE)`
(Postgres 13+) terminates any lingering connections to the target
database itself, which matters for exactly this test: a prior run's
`kill -9`'d `sms-worker` subprocess could in principle leave a dangling
connection open to its own scratch database.

The template is re-migrated whenever [`migrations_fingerprint`] of the
working tree's `backends/migrations/postgres/**` no longer matches the
fingerprint stamped onto the template the last time it was built (via
`COMMENT ON DATABASE`, checked against `pg_shdescription`) — not merely
"at most once per container lifetime" the way an earlier version of
this module worked. That earlier version asked only "does
`public.messages` exist in the template?", which stayed true forever
after the template's first-ever migration regardless of what the
migration files looked like on any later run — since the container (and
the template inside it) is deliberately left running between local
`cargo test` invocations, that let a bootstrap-only schema edit run
silently against a stale template until something forced a full
`just test-live-clean`. See [`ensure_binary_database`]'s own doc for the
fingerprint mechanism.

Postgres refuses to `CREATE DATABASE ... TEMPLATE x` while any other
session is connected to `x`, so remigrating the template always shells
out to `psql` (its own separate, self-closing connections) rather than
reusing [`ensure_binary_database`]'s own admin connection to run DDL
against it directly, and the whole ensure-template / drop-and-recreate-
binary-db sequence runs under one held advisory lock (reusing
[`MIGRATION_LOCK_NS`]/[`MIGRATION_LOCK_KEY`]) so two processes can't
interleave a template rebuild with another's `CREATE DATABASE ...
TEMPLATE`. `kill9_reclaim_live.rs` needs no change for this: it already
reads its database URL once from [`database_url`] and passes that exact
string to the `sms-worker` subprocess it spawns via `--database-url`, so
both the test and the process it kills automatically share the same
per-binary scratch database without either needing to know it's
per-binary at all.

# R1

This crate is a named R1 exception in its own right — `CONTRIBUTING.md`'s
own "Live-Postgres test harness" row — and `cargo xtask no-raw-sqlx`
allowlists this file by path. It fits the *spirit* of the "migrations"
exception too: every raw query below exists only to decide whether
`backends/migrations/postgres/**` has already been applied to a throwaway
test database, and to serialize that decision (via
`pg_advisory_lock`/`pg_advisory_unlock` — itself the *second* named
exception) against every other process racing to ask the same question.
No application row is ever read or written here.

# The `rustls` crypto provider gap the binaries' own fix didn't cover

`backends/apps/sms-gateway/src/main.rs` and `backends/apps/sms-worker/src/main.rs` each call
`rustls::crypto::ring::default_provider().install_default()` as the
first line of `main`, documented at length on that call site: dropping
`aws-lc-rs` (`AGENTS.md`'s "rustls, musl, and distroless" section) left
authkestra's own `reqwest` 0.13.4 dependency — pinned
`default-features = false, features = ["rustls-no-provider"]` — with no
crypto backend baked in, so the *first* TLS handshake it attempts
panics unless something has already installed a provider process-wide.
That fix covers every production binary, because `main` always runs.
**It does not cover test binaries**, whose entry point is the libtest
harness, not either crate's `main` — so any live suite that exercises a
code path building a `reqwest` 0.13.4 client (this workspace's own
`GatewayAuth`, via `authkestra_resource::jwt`'s `JwksCache`, is the one
found live: it fetches the issuer's JWKS over HTTP to validate a real
token, in-process, in whichever suite constructs it) panics with "No
rustls crypto provider is configured" the same way an unfixed binary
would have, and did — `backends/crates/sms-auth/tests/oidc_flow_live.rs` and
`backends/crates/sms-auth/tests/rbac_layer2_live_postgres.rs` both construct
`GatewayAuth` in-process and both panicked this way before
[`install_default_crypto_provider`] existed. Every other live suite in
this workspace was checked (`cargo test --workspace --no-fail-fast --
--ignored`, all 19 live test binaries, not just the one CI happened to
stop on — `cargo test` is fail-fast across a workspace by default,
which is why a single red binary hid the rest) and does not: the
subprocess-spawning suites (`backends/apps/sms-gateway/tests/
m1_acceptance_gate_live_postgres.rs`, `.../provision_client_cli_live_
postgres.rs`, `backends/apps/sms-worker/tests/kill9_reclaim_live.rs`) exercise
`GatewayAuth` only inside a real `sms-gateway`/`sms-worker` child
process, whose own `main` already installs the provider;
`backends/crates/sms-auth/tests/provision_app_client_live_postgres.rs` builds an
in-process OP router but never a `GatewayAuth`, so it never reaches
`authkestra_resource::jwt` at all.

[`install_default_crypto_provider`] is called unconditionally at the
top of [`database_url`] rather than left for each live suite to
remember: every live suite in this workspace already calls
`database_url` (it is how each one gets a migrated Postgres — see this
module's own doc), so hooking the install there is the one change that
reaches all of them, present and future, with no per-test-file edit and
nothing to re-discover the next time a suite starts exercising
`GatewayAuth`. `backends/apps/sms-gateway/tests/tls_no_provider_live.rs` is the
one live suite that needs no database and so never calls
`database_url` — it already installs the provider itself, for the same
reason its own module doc gives (it exists specifically to prove this
exact ordering, independently of this crate).

**Correction, authkestra 0.8.0 bump (AGENTS.md's authkestra-0.8 section):
the trigger condition this section describes — "fetches the issuer's
JWKS over HTTP" — stopped being accurate the moment that dependency
bumped, and the gap it left was not a live-suite gap this crate could
close at all.** `authkestra_resource::jwt::JwksCache` gained an
eagerly-built `client: reqwest::Client` field in 0.8.0 (0.5.4's own
`JwksCache` had no `client` field, and never touched `reqwest` before an
actual fetch — verified by reading both vendored sources directly, not
inferred) — so `reqwest::Client::new()`, and therefore the crypto-provider
panic, now fires the instant a `JwksCache` (and so a `GatewayAuth`) is
*constructed*, whether or not anything is ever fetched. Found live by
`cargo test -p sms-api --lib` (no `--ignored`, no database) failing five
tests that never perform an HTTP call at all — `sms-api`'s own
`auth::tests`/`router::tests` construct a bare `GatewayAuth` against an
unreachable JWKS URL specifically *because* they never intend to reach
the network, and none of them call [`database_url`], so
[`install_default_crypto_provider`]'s own "every live suite already
calls `database_url`" reasoning above never reached them — nor could it,
since they are not live suites at all. Fixed at the actual choke point
instead: `GatewayAuth::new` (`backends/crates/sms-api/src/auth.rs`) now installs
the provider itself, idempotently, the same `let _ = rustls::crypto::ring::
default_provider().install_default();` shape [`install_default_crypto_provider`]
already uses, and for the identical reason `cratestack-client-rust`'s own
`CratestackClient::new` does the same (AGENTS.md's "aws-lc-rs enters this
tree from authkestra alone" section) — a library that itself constructs a
`reqwest::Client` internally should not depend on every caller, live suite
or plain unit test, remembering to install a provider first. This is a
strictly stronger fix than adding another call to [`database_url`] would
have been: it protects every current and future caller of `GatewayAuth::new`
by construction, not just the ones that happen to be live-Postgres suites.
