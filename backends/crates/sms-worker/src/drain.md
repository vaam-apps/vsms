`Role::Drain`'s real body — #39. `db.events().drain()` on an interval,
per §7.1's own one-line description of this role. Singleton (§7.1):
concurrent drains are safe — the unique index on `(endpoint_id,
aggregate_id, event_type)` catches a double-insert — but every
duplicate drain is wasted work and a wasted index probe, and §8.3 says
so explicitly.

# What this role actually adds, given #38's subscribers already run
synchronously

See `sms_api::webhooks`'s own module doc for the full resolution of
"if subscribers already insert `WebhookAttempt` rows synchronously
(#38), what does this role drain?" — required reading before touching
this file. The short version: every `@@emit`-annotated mutation already
triggers an automatic post-commit drain of its own process's runtime,
so as long as `sms_api::webhooks::register_subscribers` has been
called on this process's `Cratestack` instance — which
`backends/apps/sms-worker`'s `main` does exactly once, unconditionally, before
spawning any role task, not gated on `drain` being one of `--roles` —
most events are already turned into `WebhookAttempt` rows inline, by
whatever wrote them.

What this role adds is the one thing no writer's own post-commit drain
gives you: a **write-independent** retry trigger for a row whose
handler failed on an earlier attempt (a transient error creating the
`WebhookAttempt` row, say — `drain_event_outbox` records `attempts`/
`last_error` and leaves `delivered_at IS NULL` on any handler `Err`).
Nothing else in this codebase calls `.events().drain()` on a timer —
without this role, such a row sits undelivered until the next
unrelated write happens to touch an emitting model, which "writes go
quiet" (#39's own framing, and the framework's own §8.2: "no
background drain worker exists") can leave open indefinitely.

# Alerting on oldest-undelivered age, not just on errors

#39's own acceptance line is explicit that an error count alone isn't
enough — a stalled outbox with zero errors (nothing has failed, no
`WebhookAttempt` was ever attempted because no drain ever ran) is
exactly as silent as one full of retries. [`oldest_undelivered_age`]
answers "how long has the oldest still-undelivered event been
waiting", logged every tick at `warn` once it crosses
[`STALLED_THRESHOLD`] — a log line an ops dashboard can alert on, the
same convention `lease.rs`'s own "alert on this" framing and R2's
"alert on any non-zero SM001 rate" use elsewhere in this codebase; no
metrics/alerting pipeline exists yet in this workspace to wire a
counter into instead.

**R1 exception, the fifth one.** `cratestack_event_outbox` is the
framework's own internal bookkeeping table (created lazily by
`ensure_event_outbox_table`, not one of `schema.cstack`'s models) — no
delegate exists to read it, and none of the four already-named
exceptions (migrations, `pg_advisory_lock`, `LISTEN`/`NOTIFY`,
`/readyz`'s bare `SELECT 1`) cover it either. Reading
`MIN(occurred_at) WHERE delivered_at IS NULL` here is a fifth, for the
same reason `/readyz`'s exception exists: there is no row-level policy
to bypass (the table isn't part of this schema), no audit trail to
skip (a `SELECT` isn't a mutation), and no `@version`/soft-delete
concern (it isn't a model at all). `cargo xtask no-raw-sqlx` and
`CONTRIBUTING.md`'s own R1 exceptions table both name this file.
