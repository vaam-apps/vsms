Leader election by Postgres advisory lock. §7.2 of the design doc.

**R1 exception** — one of the named ones (migrations, `pg_advisory_lock`,
`LISTEN`/`NOTIFY`). `cargo xtask no-raw-sqlx` allowlists this exact file
by path; adding a raw `sqlx::query*` call anywhere else in this crate is
still a build failure, on purpose.

# The trap this module is built to avoid

§7.2's own illustrative code sample acquires the lock connection with
`pool.acquire()` from a shared, multi-connection `PgPool`. That is exactly
the shape the doc's own prose warns against: `sqlx`'s `PoolConnection`
returns itself to the pool *on drop* rather than closing the socket, so a
dropped lease-holding connection doesn't release anything at the Postgres
level — the session stays open, recycled to serve some unrelated future
query, and the lock it silently still holds is now unreachable. No other
node can ever take that role until the whole process restarts, and
nothing about that failure is loud.

[`RoleLease`] sidesteps the trap by never using a shared pool at all: it
owns a single, dedicated [`PgConnection`] that belongs to nothing else.
Dropping it (a panic, a `kill -9`) closes the socket immediately at the
OS level; Postgres's own session-advisory-lock semantics release the lock
the moment that session ends, with zero cooperation required from this
crate or from `sqlx`. [`RoleLease::release`] is still the fast path —
signal-then-drop can lag behind an explicit unlock by however long TCP
failure detection takes — but it is a latency optimisation on top of a
mechanism that is already correct without it, not the only thing
standing between a clean release and a leak.
