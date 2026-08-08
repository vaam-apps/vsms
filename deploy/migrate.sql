-- Applies the committed migrations to $DATABASE_URL exactly once each,
-- guarded by a session-held Postgres advisory lock so two `migrate`
-- containers starting at the same time (a compose restart racing a
-- second node, a redeploy landing mid-rollout) serialise instead of
-- corrupting each other's DDL.
--
-- This does NOT regenerate migrations — see AGENTS.md's "Regenerating
-- migrations" section and #139's own acceptance note: the installed
-- `cratestack` CLI on any given machine can be newer than the pinned
-- library and emit different DDL, so nothing in the deploy path may ever
-- run `cratestack migrate diff`. This script only ever runs `\i` against
-- the exact `up.sql` files already committed under
-- schema/migrations/postgres/ — copied into the migrate image verbatim by
-- deploy/migrate/Dockerfile, never generated fresh.
--
-- `schema_migrations` is bookkeeping for this deploy path only. It is not
-- part of the committed schema/migrations tree and cratestack knows
-- nothing about it — ci/apply-migrations.sh (the CI/scratch-database path)
-- deliberately stays a dumber, unconditional loop, because CI always
-- starts from an empty database and has no "was this already applied"
-- question to answer.
--
-- hashtext('vsms_migrate') rather than a literal int: stable across runs,
-- readable in `pg_locks` next to a role name, no risk of colliding with
-- crates/sms-worker/src/lease.rs's own advisory-lock namespace (that one
-- uses classid 0x534d5300 with a small objid per role; pg_advisory_lock's
-- single-bigint form used here occupies a different, non-overlapping
-- keyspace entirely — Postgres hashes (classid,objid) and a bare bigint
-- through different code paths, so no key collision is possible between
-- the two, but they're documented as distinct on purpose regardless).
SELECT pg_advisory_lock(hashtext('vsms_migrate'));

CREATE TABLE IF NOT EXISTS public.schema_migrations (
  name        text PRIMARY KEY,
  applied_at  timestamptz NOT NULL DEFAULT now()
);

SELECT EXISTS(SELECT 1 FROM public.schema_migrations WHERE name = '0001_init') AS applied \gset
\if :applied
  \echo '0001_init already applied — skipping'
\else
  \echo 'applying 0001_init'
  \i /migrations/0001_init/up.sql
  INSERT INTO public.schema_migrations (name) VALUES ('0001_init');
\endif

SELECT EXISTS(SELECT 1 FROM public.schema_migrations WHERE name = '0002_bootstrap') AS applied \gset
\if :applied
  \echo '0002_bootstrap already applied — skipping'
\else
  \echo 'applying 0002_bootstrap'
  \i /migrations/0002_bootstrap/up.sql
  INSERT INTO public.schema_migrations (name) VALUES ('0002_bootstrap');
\endif

SELECT pg_advisory_unlock(hashtext('vsms_migrate'));

\echo 'migrations up to date'
