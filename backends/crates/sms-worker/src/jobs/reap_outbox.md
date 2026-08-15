`reap_outbox` — #42. The job kind named in §7.5's own table: "Delete
delivered `cratestack_event_outbox` rows >24h; alarm on high-`attempts`
rows." §8.2 of the design doc states the framework side of the problem
outright: "`attempts`/`last_error` are recorded but never read: no
retry cap, no backoff, no dead-letter. A permanently failing handler
retries that row forever and the table grows without bound." Confirmed
directly against the vendored source
(`cratestack-sqlx-0.7.10/src/descriptor.rs`'s `drain_event_outbox`), not
assumed from that prose: every failed delivery attempt only ever
`UPDATE`s `attempts = attempts + 1, last_error = $2` and leaves
`delivered_at` `NULL` — there is no comparison against any cap anywhere
in that function, and no code path anywhere in `cratestack-sqlx` deletes
a row of this table at all, ever. This job is the entire mechanism;
nothing upstream partially covers it.

# Reap means delete delivered rows, not poison ones — deliberately

§7.5's own table already answers "reap or quarantine": delete
**delivered** rows past retention, and separately **alarm** on
high-`attempts` (still-undelivered) rows — never delete those. That
split is the actual design decision here, not incidental phrasing:

- A **delivered** row (`delivered_at IS NOT NULL`) has already done its
  job — the event reached a `WebhookAttempt` row, or (today) had no
  subscriber registered for it at all. Keeping it past a day is pure
  bloat: nothing in this codebase ever reads a delivered outbox row
  again.
- A **poison** row (`delivered_at IS NULL`, `attempts` past the
  threshold) is the opposite: it is live evidence of a bug — a
  subscriber that keeps failing on the same event, forever, per §8.2's
  "short-circuits on the first failing handler" behaviour. Deleting it
  would erase the only record that the bug happened *and* silently drop
  the event it was trying to redeliver — a customer-visible data loss
  with no trace it ever occurred, which is a strictly worse outcome
  than "the table is a bit bigger than it should be." So a poison row
  is left exactly where it is — `attempts`/`last_error`/`occurred_at`
  untouched — and this job instead makes it loud: a `warn!` per row,
  every run, carrying `model`/`operation`/`last_error` so an operator
  can diagnose the actual subscriber bug rather than have this job
  quietly hide the symptom.

  "Quarantine" in the sense of moving the row to a separate table was
  considered and rejected: there is no schema model backing this table
  in the first place (the whole reason reading it needs an R1 exception
  — see below), so a quarantine table would just be a *second*
  hand-rolled, delegate-less, policy-less table for no benefit over
  leaving the row in place and alerting on it loudly.

# What actually constitutes a poison row

Not "any row with `attempts > 5`" on its own — that would also catch a
row still legitimately mid-retry. `drain::tick` (#39) polls every 5s
with no backoff of its own, so a row can rack up several `attempts`
within its first couple of minutes purely from that polling cadence
colliding with a slow-to-recover subscriber, not because anything is
actually stuck. The `attempts > 5` threshold from #42's own issue text
is applied only to rows that are **still undelivered**
(`delivered_at IS NULL`) — a delivered row's `attempts` count is just
"how many tries it took," not a signal of anything wrong, and this job
never alerts on it.

# R1 exception, the sixth one

Same reasoning as `drain.rs`'s own fifth exception, restated because
this is a different file: `cratestack_event_outbox` is the framework's
own lazily-created bookkeeping table (`ensure_event_outbox_table`), not
one of `schema.cstack`'s models — no delegate exists to read or write
it, so there is no row-level policy to bypass, no audit trail to skip,
no `@version`/soft-delete concern. `cargo xtask no-raw-sqlx` and
`CONTRIBUTING.md`'s own exceptions table both name this file.

Unlike `drain.rs`, this job cannot lean on `db.events().drain()` having
already run `ensure_event_outbox_table` immediately beforehand — this
job never calls `.drain()` at all, and nothing guarantees it runs after
some other write already has (a fresh deployment could have this job's
own schedule fire before the first event is ever emitted). Rather than
duplicate that table's DDL here — a second, silently-drifting copy of a
definition this crate doesn't own — both queries below treat Postgres's
`42P01` ("`undefined_table`") as "nothing to reap yet" and return success:
correct, because a table that was never created has, by construction,
no delivered rows to reap and no poison rows to alarm on.
