`Role::Scheduler`'s real body — #35. Enqueues due recurring [`Job`]
rows per §7.5's own table. Singleton (§7.1): "two schedulers
double-enqueue; `jobs_dedupe_idx` catches it, but cleanly avoiding it is
better" — the advisory lock is that clean avoidance, `dedupeKey` is
belt-and-braces underneath it, not the primary mechanism.

`expire_stale`, `reap_outbox` (#42), `purge_retention` (#67), `anchor_audit`
(#68), and, as of #64, `grey_route_watch` are registered — see
[`crate::jobs`]'s own module doc for why the remaining five §7.5 kinds
are scoped out rather than silently dropped, and
`crate::jobs::grey_route_watch`'s own doc for why that last one isn't
one of §7.5's named kinds at all.

# Cadence tracking has no dedicated schema support

§7.5's table gives each `kind` a cadence ("1 min", "1 h", "daily") but
the schema has no "last scheduled at" model — `Job` rows are the
recurring instances themselves, not a schedule record. This role tracks
"when did I last enqueue each kind" in its own process memory, seeded
at startup from each kind's most recent `Job` row (so a restart doesn't
immediately re-fire everything) and updated on every successful
enqueue. Correct as long as exactly one instance ever holds the
singleton lock — which is precisely what §7.2's advisory lock
guarantees — so there's no need for the cadence state itself to be
shared or persisted beyond that seed.

# The dedupeKey path inherits a known, already-documented gap

`try_enqueue` is a `create` + catch on `jobs_dedupe_idx`'s `23505`,
the same dedupe idiom `sms-auth`'s `SmsClientAssertionStore::record_jti`
uses — and it inherits that same crate's own documented, upstream-filed
gap: `db_sqlstate()` is unpopulated on every generated write against a
live Postgres (cratestack-sqlx `=0.5.0`, tracked as
[vymalo/vsms#87](https://github.com/vymalo/vsms/issues/87)), so a real
`23505` collision surfaces as a generic `CratestackError::Database` today, not
the `Ok(false)` this code is written to produce. This is defense in
depth under a singleton role, not the primary correctness mechanism —
see the cadence-tracking note above — so the live impact is a spurious
error log on the rare startup-race collision, not a duplicate `Job`
(the database's own unique index still blocks the insert either way;
only Rust's ability to tell *why* it failed is what's missing). Written
against the documented API, matching `sms-auth`'s own stated reasoning,
so this goes fully quiet the moment the upstream pin moves.
