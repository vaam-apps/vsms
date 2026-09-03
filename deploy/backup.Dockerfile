# syntax=docker/dockerfile:1
#
# #69's backup mechanism, rewritten in Rust (`deploy/backup-tool`) —
# replacing `deploy/{backup,restore,restore-drill,backup-entrypoint}.sh`
# entirely, a hard cutover, not a parallel path. Same reasoning
# `backends/apps/sms-migrate/Dockerfile` already used for the old
# `deploy/migrate.Dockerfile`: a small, compiled binary embeds its own
# orchestration logic instead of shelling `psql`/hand-rolled JSON out of
# a shell script — the difference here is that `pg_dump`/`pg_restore`
# themselves are a real binary dump/restore protocol nobody reimplements
# (`deploy/backup-tool/src/main.rs`'s own module doc has the full
# reasoning for what stays external and what doesn't), so this Dockerfile
# still ends on `postgres:16-alpine`, never distroless.
#
# The runtime image no longer needs `bash` or `openssl` — the restore-drill
# fallback pepper (`SMS_HASH_PEPPER:=$(openssl rand -base64 48)`) is real
# Rust now (`rand`, `deploy/backup-tool/src/drill.rs`), and there is no
# shell script left anywhere in this image for `bash` to interpret.
# `rclone` is still installed for the same reason it always was: the
# object-storage upload/download layer, deliberately provider-agnostic,
# is not something this Dockerfile reimplements either.
#
# Build context is the repository root, same as every other Dockerfile
# under backends/apps/ — build with `docker build -f deploy/backup.Dockerfile .`.

# --- builder -----------------------------------------------------------
# Same base and reasoning as backends/apps/sms-gateway/Dockerfile: Alpine's own
# libc is musl, so a plain `cargo build` here produces a static musl
# binary natively, no cross-compilation, no cross-linker.
#
# `deploy/backup-tool` is its own, separate Cargo workspace (see its own
# `Cargo.toml` header) — not part of the root workspace's `Cargo.lock` —
# so this build stage's `WORKDIR`/`COPY` scope only that directory, not
# the whole repository the way `backends/apps/*/Dockerfile` copies the root
# workspace. `--locked` still applies against this crate's own committed
# `Cargo.lock`.
FROM rust:1.95-alpine3.22 AS builder

RUN apk add --no-cache musl-dev build-base

WORKDIR /app
COPY deploy/backup-tool/ .

RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-backup-tool \
    --mount=type=cache,target=/app/target,id=cargo-target-backup-tool \
    cargo build --release --locked \
    && cp target/release/vsms-backup /tmp/vsms-backup

# --- runtime -------------------------------------------------------------
# Deliberately still root, not a `USER` drop to a non-root uid (checked as
# part of the production-readiness audit, S10, not an oversight):
# `pg_dump`/`pg_restore` need to read/write `/tmp` scratch files and
# `rclone` needs to read `/root/.config/rclone/rclone.conf` (the mount
# target `deploy/docker-compose.yml`'s `backup` service already uses) —
# both assume the container's default `$HOME=/root`, which only holds for
# uid 0 on this Alpine-derived base with no `/etc/passwd` entry created
# for anything else. Revisit alongside a real non-root uid/`$HOME`/rclone
# config-path plumbing change, not as a one-line `USER` addition that
# would silently break the restore-drill's own scratch-directory and
# rclone-config assumptions.
FROM postgres:16-alpine AS runtime

RUN apk add --no-cache rclone

COPY --from=builder /tmp/vsms-backup /usr/local/bin/vsms-backup

# Production-readiness audit S10: a real health signal for the
# long-running `schedule` container, not just "is the process alive" —
# `vsms-backup healthcheck` (`schedule::check_health`'s own module doc has
# the full mechanism) fails once the last *successful* backup is older
# than 2x the schedule's own period, not merely once the process has
# crashed. `--start-period` is generous: a fresh container's first backup
# can genuinely take a while against a large database, and this check
# reports unhealthy until that first backup lands, by design — see
# `check_health`'s own doc for why that's correct, not a bug.
HEALTHCHECK --interval=15m --timeout=10s --start-period=15m --retries=2 \
  CMD ["/usr/local/bin/vsms-backup", "healthcheck"]

# `schedule` is the long-running entrypoint (an initial backup unless
# `BACKUP_RUN_ON_START=false`, then one per `BACKUP_CRON_SCHEDULE` tick,
# forever, until SIGTERM/SIGINT — see `schedule.rs`'s own module doc,
# including the PID-1-signal-disposition trap this replaces `crond`
# without reintroducing). `backup`/`restore`/`restore-drill` are run
# ad hoc against this same image, e.g.:
#   docker run --rm <this image> vsms-backup restore-drill \
#     --yes-i-understand-this-destroys-the-target-database
ENTRYPOINT ["/usr/local/bin/vsms-backup"]
CMD ["schedule"]
