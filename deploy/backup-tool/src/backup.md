`backup` — takes a single `pg_dump` of `DATABASE_URL` in custom
format, writes the manifest next to it, pushes both to the configured
`rclone` remote, and prunes anything past the retention window. Direct
port of the old `deploy/backup.sh`; see that file's own git history
for the shell version this replaces and `docs/runbooks/backup-restore.adoc`
for the full "why pg_dump, not WAL archiving" reasoning, unchanged by
this rewrite.
