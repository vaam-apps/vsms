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

A claim is also decided by **which endpoints it may take attempts from**. A `hooks`
worker without the envelope signing key claims `hmac` attempts only
([`ServedModes`]), and no worker takes the attempts of an endpoint that is
deactivated or whose circuit is open. Both are conditions of the rows the query
selects, never a filter over what it returned: a window of the oldest rows is a
fixed number of rows, and an endpoint whose attempts the worker will not take, with
that many due, fills it and starves everyone behind it. It is also why nothing is
written for a row the worker cannot serve, which a "claim it, find out, hand it back"
design could not say. [`claim_webhook_attempts`] is that claim;
[`count_sealed_attempts_waiting`] is the `COUNT(*)` a keyless worker reports with.

# What a claim costs, and why it is not always one query

The claim reads the endpoints first (`webhook_endpoints`, a scan of the few rows it
holds) and chooses, with one rule for both modes (`ServedModes::can_claim`: the
endpoint is active, its circuit is closed, and for a keyless worker it delivers
`hmac`):

- **Every endpoint in the table is claimable by this worker**: the endpoint
  condition would remove nothing, so the claim is one time-ordered read of
  `webhook_due_idx` and, for what is left of `budget`, one of
  `webhook_attempts_lease_reclaim_idx` (`time_ordered_candidates`), each stopped
  by `LIMIT` at `budget` rows whatever the number of endpoints. This is every
  healthy all-`hmac` deployment (every deployment until an operator switches one
  endpoint to `sealed`) and every healthy keyed one. The rule is "every endpoint,
  in any state", not "no sealed endpoint": see below.
- **Otherwise**: one query per claimable endpoint, `endpoint_id = $1`, which
  `webhook_endpoint_due_idx (endpoint_id, next_attempt_at) WHERE state IN
  ('pending','failed')` (migration `0011`) answers from its first entry, in order,
  stopped by `LIMIT budget` (`per_endpoint_candidates`). The per-endpoint lists are
  merged by `nextAttemptAt`, so the oldest-first order is the one the single query
  has, and the expired leases are one `endpoint_id IN (...)` query over the same
  endpoints.

**Why the single query is only for a table with nothing to leave out.** A window of
the oldest rows of any endpoint is a fixed number of rows. An endpoint a claim must
not take from (deactivated after a 410, a circuit open, `sealed` for a keyless
worker) can hold that many due attempts, and nothing purges them
(`purge_retention` deletes messages and receipts, not attempts). The claim used to
fetch the oldest `3 x budget` rows and drop those of unhealthy endpoints
afterwards, so such an endpoint with 60 or more due attempts was all of every
claim's window: the claim returned nothing on every tick, for as long as the
endpoint stayed that way, and delivery stopped for every app. Keyed workers had this
from the first version; the keyless all-`hmac` fallback brought it back for keyless
ones. Reproduced in SQL (case (d) below): `ORDER BY next_attempt_at LIMIT 60` over a
table where one inactive endpoint held the oldest due rows returned 60 rows, all of
that endpoint's. The fan-out never asks about an endpoint it may not take from. The
price of the rule is that one deactivated endpoint, kept for its history, keeps a
deployment on the fan-out for as long as the row exists.

**The fan-out costs per endpoint, not per poll.** Each endpoint's query returns up
to `min(its due backlog, budget)` full rows (payloads included), so a deployment
with `N` backlogged claimable endpoints on the fan-out reads and ships up to
`N x budget` rows a poll for the `budget` it keeps. Each is asked for `budget`: no
endpoint can contribute more than `budget` rows to the oldest `budget` of the merge.

**There is no over-fetch.** The claim used to read three times `budget` rows so that
dropping the rows of unhealthy endpoints afterwards still tended to fill the batch.
That bought nothing once those endpoints were left out before the rows were read, and
cost three times the rows, payloads included, on every poll of every worker. What
remains to drop is a row whose endpoint changed between the endpoints' read and the
re-read just before the leases (`filter_by_endpoint_health`, which takes the
worker's mode into account too): an under-filled poll the next one does not repeat,
because it reads the endpoint's new state before it reads any row. That window is the
milliseconds between two reads and the compare-and-swap on the attempt, not the
length of a poll.

`EXPLAIN (ANALYZE, BUFFERS)` of the exact statements the claim sends (captured with
`log_statement = all`, parameters substituted), PostgreSQL 16.15, `budget` 20,
400-byte payloads, attempts inserted in time order so that an endpoint's due rows
are spread over the heap as they are in production. 50 `hmac` endpoints with 2 000
due attempts each, interleaved in time, and a tenth as many `succeeded` rows as
history; `sealed` ones (20) are due *before* every `hmac` attempt where a case says
so. Buffers are `shared hit + read`, the time is the warm execution time summed over
the statements, "rows" are `webhook_attempts` rows returned, and "statements" are the
`SELECT`s of the claim (the endpoints, the attempts, the re-read of the claimed
rows' endpoints). "Round 6" is the claim as `fdd5f79` has it (a single query of
`3 x budget` rows, for a table the keyless claim thought safe, and for every keyed
claim), "round 7" is this one. The round 6 rows of (d) and (e) are
the measured attempts `SELECT` plus the two endpoint reads of round 7 (2 buffers,
0.06 to 0.1 ms each), since that claim is no longer in the tree; (a) and (c) are
round 6's own figures. Times come from different runs: compare the buffers.

| case                                                                         | claim   | statements | attempt rows | buffers | time     | claimed |
| ---------------------------------------------------------------------------- | ------- | ---------: | -----------: | ------: | -------: | ------: |
| (a) keyless, all `hmac`, healthy: 50 endpoints x 2 000 due                   | round 6 |          3 |           60 |      17 |  0.14 ms |      20 |
| (a) keyless, all `hmac`, healthy: 50 endpoints x 2 000 due                   | round 7 |          3 |           20 |       9 |  0.38 ms |      20 |
| (c) keyless: 200 000 `sealed` due, 50 `hmac` x 2 000 due                     | round 6 |         52 |        1 000 |   1 169 |  4.34 ms |      20 |
| (c) keyless: 200 000 `sealed` due, 50 `hmac` x 2 000 due                     | round 7 |         52 |        1 000 |   1 176 | 16.14 ms |      20 |
| (d) keyless, all `hmac`: 50 x 2 000 due, one inactive `hmac` with 200 000    | round 6 |          3 |           60 |      12 |  0.34 ms |   **0** |
| (d) keyless, all `hmac`: 50 x 2 000 due, one inactive `hmac` with 200 000    | round 7 |         52 |        1 000 |   1 176 | 17.58 ms |      20 |
| (e) keyed, all endpoints healthy: 50 `hmac` x 2 000, 20 `sealed` x 100       | round 6 |          2 |           60 |       8 |  0.16 ms |      20 |
| (e) keyed, all endpoints healthy: 50 `hmac` x 2 000, 20 `sealed` x 100       | round 7 |          3 |           20 |       9 |  0.37 ms |      20 |

(a) is the single query: 20 rows however many endpoints there are, and the
endpoints' own two reads. (c) is the fan-out, unchanged by this round (the same
statements; the buffers differ by the table's state after the runs): still
`N x budget` rows, the reason a deployment with many backlogged `hmac` endpoints and
one `sealed` one should not be called cheap. **(d) is the finding**: one endpoint
deactivated after a 410 with 200 000 due attempts older than everyone's. The round 6
claim read its 60 oldest rows, every one the inactive endpoint's, dropped them and
returned **nothing**, on every tick, while 100 000 attempts of healthy endpoints
waited. The fan-out is the price of getting out of that: it costs what (c) costs, and
only for as long as an endpoint is in that state. (e) is a keyed worker over healthy
endpoints of both modes: the single query, with one more statement than before (the
endpoints) and a third of the rows. What the fan-out removed from the cost of a
keyless claim is the sealed backlog: the `EXISTS` query against it, `hmac`
attempts due after 200 000 and 2 000 000 sealed ones, three `hmac` endpoints holding
five due attempts between them:

| due sealed attempts | claim                        | buffers                           | time                  |
| ------------------- | ---------------------------- | --------------------------------- | --------------------- |
| 200 000             | before (one query, `EXISTS`) | 13 929                            | 101.6 ms              |
| 200 000             | fan-out (3 endpoint queries) | 4 + 4 + 4 (+ 1 for the endpoints) | 0.09 + 0.02 + 0.02 ms |
| 2 000 000           | before                       | 64 253                            | 877 ms                |
| 2 000 000           | fan-out                      | 4 + 4 + 4 (+ 1)                   | 0.10 + 0.05 + 0.03 ms |

The fan-out's cost does not move with the **sealed** backlog; it moves with the
number of claimable endpoints and with how much each holds, up to `budget` rows
apiece (4 buffers for an endpoint with a handful of due rows, 23 or 24 for one with more
than `budget`). The commit message of each round carries its figures.

**Why one query per endpoint and not `endpoint_id IN (...)`**, which looks the
same and is the obvious design: with a list the planner may read `webhook_due_idx`
in time order and filter, and it does, because its statistics cover the whole
table, history included, and make any endpoint with a long past look like a few
percent of the rows. With `webhook_endpoint_due_idx` present, the `IN` form still
read 64 207 buffers in 604 ms at 2 000 000 rows. An equality leaves that plan
little to win with: next to a sealed backlog the planner reads the endpoint's own
rows from `webhook_endpoint_due_idx`, never the backlog. It is a cost choice and
not a guarantee: where an endpoint holds a few percent of the table and no backlog
stands ahead of it (40 `hmac` endpoints, 1 000 due each, and nothing else), the
planner may walk `webhook_due_idx` instead (about 800 entries a query for a
`budget` of 20), which is then the cheaper plan by its estimate. The planner kept that plan
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
(4 ms) at 200 000. On the fan-out it stays one `endpoint_id IN (...)` query, and it keeps the
lease index for itself; a second index on `(endpoint_id, lease_until)` would add a
write to every claim for a pile that cannot grow past the attempts in flight.

One cost is not removed: within an endpoint the index is ordered by
`nextAttemptAt`, and the `nextAttemptAt <= now` condition is a filter (it is
`IS NULL OR ...`), so an endpoint with a very long run of `failed` attempts
resting out a backoff is read to the end of that run on each poll. That is the
`hmac` work itself, and `webhook_due_idx` does the same.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
