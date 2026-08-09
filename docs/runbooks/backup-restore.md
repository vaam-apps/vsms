# Backup and restore — and the drill that proves it works

[#69](https://github.com/vymalo/vsms/issues/69)'s own framing is the right
one: **backups that have never been restored are not backups.** This
runbook is not "how to configure a backup job" — it is that, plus the
scripted drill that proves the result is actually restorable, and what a
successful drill looked like the one time it was actually run end to end
while building this.

## Decision: pg_dump, not WAL archiving

[`docs/architecture.md` §9.2](../architecture.md#92-deployment) mentions
"Postgres with WAL archiving to object storage plus nightly `pg_dump`" in
the same breath. This deployment builds only the second half, and that is
a deliberate choice, not a shortcut waiting to be finished:

- **This is a single VM, pre-production, with no live traffic yet.**
  `AGENTS.md` records that as a standing fact of the current state, not a
  guess — there is no live database anywhere in this repository's history
  so far. WAL archiving buys point-in-time recovery (restore to any
  second, not just the last nightly dump) at the cost of a second,
  always-on pipeline: `archive_command` wired into the Postgres
  container, a WAL-shipping agent watching it, object-storage retention
  for the WAL stream *and* periodic base backups, and a restore procedure
  that has to replay WAL forward from a base backup rather than run a
  single `pg_restore`. That pipeline protects against losing minutes of
  data since the last dump — real value once there's real traffic to lose,
  none yet.
- **A once-daily `pg_dump` plus a *proven* restore path is the honest tool
  for this deployment's actual size.** It has one moving part
  (`pg_dump` → `rclone` → object storage) instead of several, and this
  runbook's own drill (below) is the reason "honest" is the right word
  here rather than "adequate" — the mechanism was actually exercised, not
  assumed to work because the command exits zero.
- **The upgrade path is documented, not foreclosed.** When real traffic
  and a real RPO requirement exist, `archive_command = 'rclone copyto %p
  ...'` (or a dedicated tool — `pgbackrest`, `wal-g`) layers on top of
  this without touching `backup.sh`/`restore.sh`: the manifest format
  here (`taken_at`, `schema_migrations_applied`, a pepper fingerprint) is
  a base-backup metadata scheme PITR tooling would reuse, not replace.
  Revisit this decision when the recovery-point objective actually
  matters, not preemptively.

The daily dump's own recovery-point objective is what it is: **up to 24h
of data loss in the worst case** (a failure right before the next
scheduled run). That is the number this decision accepts. If that
stops being acceptable before WAL archiving lands, shortening
`BACKUP_CRON_SCHEDULE` is a one-line mitigation; it does not close the
gap to zero the way PITR would.

## Where backups go

**Not the same disk as the database.** `deploy/docker-compose.yml`'s
`postgres_data` volume and a same-host backup directory fail together —
disk failure, host loss, or a `docker volume rm` typo takes both out at
once. This is why the mechanism here is object storage from the start,
not a local directory with "move it to S3 later" as a TODO.

**The bucket is the operator's choice, not a hardcoded vendor.** The
`backup` service (added to `deploy/docker-compose.yml` by this PR) uses
[`rclone`](https://rclone.org) rather than a vendor-specific CLI, so the
same `deploy/backup.sh`/`deploy/restore.sh` work unmodified against AWS
S3, Backblaze B2, Google Cloud Storage, Azure Blob, MinIO, or (for a drill
only — see below) a bare local path. That is a required, documented
configuration, not a silently-local default:

- `BACKUP_RCLONE_REMOTE` (`deploy/.env.example`) has **no default**. The
  `backup` service's own `${BACKUP_RCLONE_REMOTE:?...}` in
  `docker-compose.yml` means it refuses to start rather than quietly
  backing up to nowhere.
- The remote's own credentials live in `deploy/secrets/rclone.conf`
  (rclone's own config file format — `rclone config` writes it
  interactively, or hand-write it per
  [rclone's docs](https://rclone.org/docs/#config-config-file)),
  gitignored and mounted read-only into the `backup` container — the same
  convention `docs/runbooks/deployment.md`'s "Secrets" section already
  uses for `deploy/secrets/console-private-key.pem`, including its
  documented threat model (protects against git history / image layers /
  `docker inspect`; does not protect against host filesystem access).

**Cameroon hosting note.** `AGENTS.md`'s open question #1 (Law No.
2024/017, cross-border personal-data transfer) applies to backups exactly
as much as it applies to the live database — a `msisdn`/`msisdnHash`
column doesn't stop being personal data because it's inside a `.dump`
file in a bucket. Whatever the eventual answer to "where does the
database live" is, the backup bucket's region has to match it. Not
resolved here; flagging it so the bucket choice doesn't quietly reopen a
question §10 says needs a lawyer.

## The mechanism

Three scripts, all under `deploy/`, all shipped inside
`deploy/backup.Dockerfile` (`postgres:16-alpine` + `rclone` + `openssl` —
the exact `pg_dump`/`pg_restore` this stack's own Postgres major version
needs; unlike `app/sms-migrate`, this one has no substitute for a real
`postgres` client toolchain, so it stays on that base image rather than
distroless):

- **`backup.sh`** — `pg_dump --format=custom` (pg_restore's own format:
  built-in compression, selective/parallel restore — a plain SQL dump
  gets none of that), plus a small **unencrypted** manifest next to it
  (`taken_at`, `postgres_version`, `schema_migrations_applied`, and a
  pepper fingerprint — see below), pushed to `$BACKUP_RCLONE_REMOTE` via
  `rclone copyto`, then prunes anything in that remote older than
  `$BACKUP_RETENTION_DAYS` (default 30) via `rclone delete --min-age`.
  Scheduled by the `backup` compose service via Alpine's own busybox
  `crond` (`$BACKUP_CRON_SCHEDULE`, default `0 3 * * *` UTC) — no extra
  scheduler daemon, since one is already in the base image. Also runs
  once immediately on container start (`$BACKUP_RUN_ON_START=true` by
  default) so a fresh deploy has a restorable backup right away rather
  than waiting up to 24h for the first scheduled tick.
- **`restore.sh`** — the reusable restore primitive. Pulls a named
  backup, `--latest`, or a `--local <path>` dump already on disk, checks
  its manifest's pepper fingerprint against the caller's own
  `$SMS_HASH_PEPPER` (warns, never blocks — see "the pepper is part of
  the recoverable state" below), and runs `pg_restore --clean --if-exists
  --no-owner --no-privileges` into `$DATABASE_URL`. `--clean` drops and
  recreates every object *inside* the target database; it never touches
  the database itself, so this is safe to point at an empty or
  already-populated database alike.
- **`restore-drill.sh`** — the actual gate. Seeds a recognisable marker
  row, records an *exact* row count for every table (a real `SELECT
  count(*)` per table, not `pg_stat_user_tables`'s `n_live_tup` estimate),
  runs `backup.sh`, destroys everything in the target database, runs
  `restore.sh`, and diffs both the row counts and the marker row's exact
  content before vs after. Non-zero exit means the *mechanism* is broken,
  not that the drill script itself failed for some unrelated reason.
  Requires an explicit
  `--yes-i-understand-this-destroys-the-target-database` flag and reads
  `$DATABASE_URL` — there is no default target, on purpose. Run it
  against a scratch database, never production.

Why `DROP SCHEMA public CASCADE; CREATE SCHEMA public;` rather than a real
`dropdb`/`createdb` as the drill's destructive step: it needs only the one
`$DATABASE_URL` connection the drill already has (no separate
superuser/admin connection to a different database to issue `DROP
DATABASE`), and it works against any Postgres, including a managed one
where the application's own role can never hold `DROP DATABASE` — an
honest stand-in for "destroy the database," not a shortcut around it. A
production incident restoring after a genuinely lost VM or corrupted data
directory is a different, larger procedure (provision a new Postgres,
`CREATE DATABASE`, then `restore.sh --latest`) — the drill proves the
`pg_dump`/`pg_restore` round trip is sound; it does not simulate losing
the VM itself.

## The pepper is part of the recoverable state, not just the database

`Message.msisdnHash`/`Message.bodyHash` are `HMAC-SHA256` keyed by
`SMS_HASH_PEPPER` (`crates/sms-api/src/pepper.rs`, landed in #134). A
`pg_dump` backup captures the *hashes*, never the plaintext `msisdn`/body
that produced them (that's the point of hashing them) — which means the
backup is only useful for opt-out matching and dedupe if it is restored
alongside the **same pepper** that was active when the dump was taken.
Restore the same rows under a *different* pepper and every
`msisdnHash`/`bodyHash` silently stops matching anything — not an error,
just rows that look like "never opted out" / "not a duplicate" to code
that only ever compares hashes computed under whatever pepper is
currently configured. `pepper.rs`'s own module doc calls this the
"rotation consequence"; a restore under the wrong pepper is the same
failure mode by a different route.

Two things follow from that, both implemented here:

1. **The pepper must be backed up too, separately from the database, and
   restorable to the same place at the same time as a database restore.**
   This runbook does not solve secret-backup for you — `SMS_HASH_PEPPER`
   belongs wherever the operator already keeps `POSTGRES_PASSWORD` and
   `ORANGE_CM_CLIENT_SECRET` (a password manager, a sealed secret, before
   `docs/runbooks/deployment.md`'s own eventual `sops` migration). The
   concrete instruction: **whatever backs up `deploy/.env`, back it up on
   the same cadence as the database, and treat "restore the database" and
   "restore the matching `.env`" as one operation, not two.**
2. **Every backup carries a *verifiable* fingerprint of the pepper it was
   taken under, never the pepper itself.** `backup.sh`'s manifest stores
   `sha256(SMS_HASH_PEPPER)` in the clear. That is deliberately safe to
   leave unencrypted: `HashPepper`'s own minimum is 32 bytes of `openssl
   rand` output, so — unlike a raw MSISDN hash, brute-forceable in seconds
   over Cameroon's ~10^7-candidate numbering space (`AGENTS.md`'s #134
   section) — a fingerprint of 256 real bits of entropy is not
   meaningfully brute-forceable. `restore.sh` hashes the caller's own
   `$SMS_HASH_PEPPER` the same way and compares; a mismatch **warns
   loudly and proceeds** rather than blocking, because restoring under a
   deliberately different pepper is a legitimate choice in some DR
   scenarios (e.g. a compromised pepper being rotated as part of the same
   incident) — it must never be a *silent* one.

## Running the drill

The drill this PR's own Verification section reports was run this way —
against a disposable `postgres:16-alpine` container, using the exact
`deploy/backup.Dockerfile` image (so the `pg_dump`/`pg_restore`/`rclone`
versions match what production actually runs), on a dedicated Docker
network rather than exposing the target Postgres to the drill container
over the host network:

```bash
docker network create vsms-69-net
docker run -d --name vsms-69-pg --network vsms-69-net -p 15503:5432 \
  -e POSTGRES_USER=vsms -e POSTGRES_PASSWORD=vsms -e POSTGRES_DB=vsms \
  postgres:16-alpine

DATABASE_URL=postgres://vsms:vsms@localhost:15503/vsms ./ci/apply-migrations.sh

docker build -f deploy/backup.Dockerfile -t vsms-backup-drill .

docker run --rm --network vsms-69-net \
  -e DATABASE_URL=postgres://vsms:vsms@vsms-69-pg:5432/vsms \
  -e BACKUP_RCLONE_REMOTE=<a real remote, or omit for a throwaway local one> \
  --entrypoint /restore-drill.sh \
  vsms-backup-drill \
  --yes-i-understand-this-destroys-the-target-database
```

Omitting `BACKUP_RCLONE_REMOTE` makes `restore-drill.sh` use a throwaway
local directory of its own (`mktemp -d`) so the drill has zero external
dependencies by default; passing a real `rclone` remote (with
`deploy/secrets/rclone.conf` mounted the same way the `backup` service
mounts it) exercises the actual upload/download path against real object
storage instead, which is the stronger drill to run periodically once a
real remote exists.

**Run it again periodically, not just once.** A drill that passed the day
this PR merged says nothing about whether a schema change six months from
now broke `pg_restore`'s ability to replay it, or whether an operator's
`rclone.conf` silently expired. `restore-drill.sh` is designed to be
re-run by hand or wired into a scheduled job against a disposable
database — no design for automatically scheduling it exists yet; that is
a reasonable follow-up once this deployment has somewhere non-production
to run it against on a cadence.

## Verification actually performed for this PR

Run exactly as shown above, with two additional passes beyond the bare
minimum: once seeding only a marker table, and again after adding a real
domain row (a `providers` row with recognisable, non-default field
values) and pointing `BACKUP_RCLONE_REMOTE` at a *named* rclone remote
(`type = local` in a mounted `rclone.conf`, not the bare-local-path
shortcut) so the actual `rclone lsf`/`copyto` code paths — the ones a real
S3 remote would exercise — were proven, not bypassed.

| Step | Result |
|---|---|
| `providers` row inserted (`key='orange_cm_drill'`, `cost_per_segment_xaf=12.5`, `max_tps=10`, `healthy=true`) before the drill | present |
| Row counts across all 19 domain tables + `schema_migrations` + the marker table, before vs after | **identical**, table-by-table exact `count(*)` — e.g. `operator_prefix_rules=14`, `message_state_transitions=25`, `job_state_transitions=8`, `providers=1` |
| Marker row (`backup_drill_marker`) content, before vs after | identical (`note = 'restore-drill-proof'`) |
| Spot-checked `providers` row, before vs after | identical on every checked column — `id`, `key`, `display_name`, `cost_per_segment_xaf`, `healthy`, `max_tps` all round-tripped exactly |
| `restore-drill.sh`'s own exit code | `0`, printing `RESTORE DRILL PASSED` |
| Pepper fingerprint check, matching pepper | `restore.sh: pepper fingerprint matches` |
| Pepper fingerprint check, deliberately wrong pepper (negative test) | `restore.sh: WARNING — SMS_HASH_PEPPER does not match...`, restore still completed (warn, not block — as designed) |
| Retention pruning (`BACKUP_RETENTION_DAYS=0`, forcing everything to qualify as "older than 0 days") | confirmed deletion actually happens, not just a clean exit: the backup directory was empty immediately after the run that produced it |

**One real bug found live, not by inspection:** the first version of
`restore-drill.sh` located the dump `backup.sh` had just produced with a
raw `find` against `$BACKUP_RCLONE_REMOTE`. That only worked by accident
when the remote happened to be a bare local path — pointed at a *named*
rclone remote (`drilllocal:/backups`), `find` saw a literal string
containing a `:` and found nothing, and the drill failed at "backup.sh
did not produce a .dump file" despite the backup having genuinely
succeeded. Fixed to use `rclone lsf` (matching how `restore.sh` itself
already listed remote backups) and to hand the restore step the dump
*name* rather than a synthesised local path, so `restore.sh` pulls it
through `rclone` the same way a real restore would. Re-run after the fix,
against the same named remote, and it passed — see the table above.

**Not verified:**

- A real cloud object-storage backend (S3, B2, GCS, Azure) — only
  `rclone`'s own `local` backend, both as a bare path and through a named
  remote in a config file. The `rclone copyto`/`lsf`/`delete` calls
  `backup.sh`/`restore.sh` use are backend-agnostic by rclone's own
  design, and the named-remote pass above proves the config-file-driven
  path (not just the "happens to be a filesystem path" shortcut), but a
  real network round trip to an actual bucket was not exercised in this
  environment.
- `deploy/docker-compose.yml`'s `backup` service running for a real
  `BACKUP_CRON_SCHEDULE` tick under `crond` — the entrypoint's
  `BACKUP_RUN_ON_START` path was exercised indirectly (this drill invokes
  `backup.sh` directly, the same script the cron job runs, but not
  through `crond`/the entrypoint script itself).
- Restoring into a *different*, freshly created database/VM standing in
  for "the original VM is gone" — the drill proves the dump/restore round
  trip against the same running Postgres instance, not a full
  disaster-recovery rehearsal that also re-provisions Postgres itself.
- Scheduled/automated re-running of the drill itself (see "Run it again
  periodically" above) — no cron/CI wiring exists yet for
  `restore-drill.sh`.
