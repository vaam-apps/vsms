# syntax=docker/dockerfile:1
#
# The one-shot migration runner (#139's "which component applies
# migrations, when, and what happens if two instances start at once").
# Build context is the repository root — build with
# `docker build -f deploy/migrate.Dockerfile .` — because it needs
# schema/migrations/, committed and never regenerated here (see
# deploy/migrate.sql's own header and AGENTS.md's "Regenerating
# migrations": an installed `cratestack` CLI newer than the pinned library
# emits different DDL, so nothing in this path may run `migrate diff`).
#
# postgres:16-alpine, not a from-scratch image: it already carries the
# exact `psql` this needs, at the exact server major version the compose
# stack runs, with `\gset`/`\if` meta-command support — no separate client
# install, and no version-skew question between the tool applying
# migrations and the server receiving them.
FROM postgres:16-alpine

COPY schema/migrations/postgres /migrations
COPY deploy/migrate.sql /migrate.sql
# #153: the cratestack_idempotency bookkeeping table's DDL — see
# ci/idempotency-table.sql's own header and migrate.sql's `\i` of this path.
COPY ci/idempotency-table.sql /idempotency-table.sql

ENTRYPOINT ["sh", "-c", "psql \"$DATABASE_URL\" -v ON_ERROR_STOP=1 -f /migrate.sql"]
