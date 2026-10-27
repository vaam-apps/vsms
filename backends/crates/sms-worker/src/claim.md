The CAS claim loop every claiming role shares. §7.3 of the design doc.

`SKIP LOCKED` is not expressible through the framework — verified by grep
across every crate and by compile error on `skip_locked()` (§7.3).
`.for_update()` exists but blocks rather than skips, which for a claim
loop means workers queuing behind each other instead of moving on. So
every claim here is optimistic CAS on `@version` instead: select
candidates, take a lease with `if_match(version)`, and read the outcome —
`PreconditionFailed` means another worker won a race that was always
going to have a loser, `Forbidden` means something worth knowing about
happened, and anything else is a real failure.

[`Claimable`] is what makes [`claim_batch`] one function rather than one
per model — "the job and webhook claims are the same function with
different types" (§7.3). This module implements it for [`Message`]
(`dispatch`'s claim, based on §7.3's own worked example, with two
corrections — see the doc comment on its `candidates` impl), for [`Job`]
(`jobs`'s claim, #35), and for [`WebhookAttempt`] (`hooks`'s claim, M3 #40,
see that `impl`'s own doc for how it differs from the other two: endpoint
health, not just row state, decides what's claimable).

A claim is also decided by **which delivery modes the claiming worker can serve**
([`ServedModes`]). A `hooks` worker without the envelope signing key claims
`hmac` attempts only, and that is a condition of the rows the query selects, never
a filter over what it returned: the over-fetch window is a fixed number of rows,
and a sealed backlog the worker will not take would fill it and starve the `hmac`
rows behind it. It is also why nothing is written for a row the worker cannot
serve, which a "claim it, find out, hand it back" design could not say.
[`claim_webhook_attempts`] is that claim; [`count_sealed_attempts_waiting`] is the
`COUNT(*)` a keyless worker reports with.

# What a keyless claim costs, and why it is not one query

The first version expressed the condition as a subquery on the attempt's endpoint
(`EXISTS (SELECT 1 FROM webhook_endpoints ... AND delivery_mode = 'hmac')`).
Postgres cannot push that into `webhook_due_idx (next_attempt_at)`: it reads the
index in time order and looks every entry's endpoint up until it has
`fetch_budget` `hmac` rows. With few `hmac` rows that is the whole sealed backlog,
on every poll of every keyless replica. The keyless claim now reads the
`hmac` endpoints first (`claimable_hmac_endpoints`, a scan of the few rows
`webhook_endpoints` holds) and asks for each one's due attempts with
`endpoint_id = $1`, which `webhook_endpoint_due_idx (endpoint_id, next_attempt_at)
WHERE state IN ('pending','failed')` (migration `0011`) answers from its first
entry, in order, stopped by `LIMIT`. The per-endpoint lists are merged by
`nextAttemptAt`, so the oldest-first order is the one the keyed claim has.

`EXPLAIN (ANALYZE, BUFFERS)` of the exact statements the claim sends, PostgreSQL
16, with the `hmac` attempts due *after* the sealed ones (the worst order), 20
sealed endpoints, three `hmac` ones holding five due attempts, and a tenth as many
`succeeded` rows as sealed ones for history. Buffers are `shared hit + read`; the
2 000 000 rows case has a 100-byte payload to fit the disk it was measured on, the
other 400:

| due sealed attempts | claim                        | buffers                           | time                  |
| ------------------- | ---------------------------- | --------------------------------- | --------------------- |
| 200 000             | before (one query, `EXISTS`) | 13 929                            | 101.6 ms              |
| 200 000             | after (3 endpoint queries)   | 4 + 4 + 4 (+ 1 for the endpoints) | 0.09 + 0.02 + 0.02 ms |
| 2 000 000           | before                       | 64 253                            | 877 ms                |
| 2 000 000           | after                        | 4 + 4 + 4 (+ 1)                   | 0.10 + 0.05 + 0.03 ms |

The after rows do not move with the backlog; the endpoints count is what grows
them, linearly: one index probe each, per poll, per keyless replica. The message of the commit that added the index
carries the figures.

**Why one query per endpoint and not `endpoint_id IN (...)`**, which looks the
same and is the obvious design: with a list the planner may read `webhook_due_idx`
in time order and filter, and it does, because its statistics cover the whole
table, history included, and make any endpoint with a long past look like a few
percent of the rows. With `webhook_endpoint_due_idx` present, the `IN` form still
read 64 207 buffers in 604 ms at 2 000 000 rows. An equality leaves that plan
nothing to win with: the per-endpoint index reads exactly the rows asked for, so
it never costs more than the time-ordered index. The planner kept that plan
through twelve executions of the prepared statement, `sqlx`'s way, under the
default `plan_cache_mode`. (A generic plan forced with `plan_cache_mode =
force_generic_plan` reads the endpoint's own rows through `webhook_attempts_dedupe`
instead, 5 982 buffers for an endpoint with 6 668 rows of history: the default
mode takes a generic plan only when it is no dearer than the custom ones, and this
one is dearer.)

The index does not help the **reclaim** read (`state = 'delivering' AND
lease_until < now`) and does not need to. `webhook_attempts_lease_reclaim_idx`
holds `delivering` rows only, so that read costs what is in flight, never the due
backlog: with 605 attempts abandoned by a crashed worker it read 22 buffers
(0.2 ms) at 2 000 000 due rows, and with a stress pile of 20 000 it read 1 485
(4 ms) at 200 000. It stays one `endpoint_id IN (...)` query, and it keeps the
lease index for itself; a second index on `(endpoint_id, lease_until)` would add a
write to every claim for a pile that cannot grow past the attempts in flight.

One cost is not removed: within an endpoint the index is ordered by
`nextAttemptAt`, and the `nextAttemptAt <= now` condition is a filter (it is
`IS NULL OR ...`), so an endpoint with a very long run of `failed` attempts
resting out a backoff is read to the end of that run on each poll. That is the
`hmac` work itself, and `webhook_due_idx` does the same.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
