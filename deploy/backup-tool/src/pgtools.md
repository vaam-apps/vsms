`pg_dump`/`pg_restore` themselves — real binary-format dump/restore
tools nobody reimplements in application code, kept as external
processes on purpose (same posture `app/sms-migrate`'s own module doc
takes toward `psql \i`: replace the *orchestration*, never the
database engine's own tooling). The runtime image still ships
`postgres:16-alpine` for exactly these two binaries — see this crate's
own `Dockerfile`.
