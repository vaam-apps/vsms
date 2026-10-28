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

# What a keyless claim costs, and why it is not always one query

The first version expressed the condition as a subquery on the attempt's endpoint
(`EXISTS (SELECT 1 FROM webhook_endpoints ... AND delivery_mode = 'hmac')`).
Postgres cannot push that into `webhook_due_idx (next_attempt_at)`: it reads the
index in time order and looks every entry's endpoint up until it has
`fetch_budget` `hmac` rows. With few `hmac` rows that is the whole sealed backlog,
on every poll of every keyless replica. The keyless claim now reads the endpoints
first (`webhook_endpoints`, a scan of the few rows it holds) and chooses:

- **No `sealed` endpoint exists, in any state** (every deployment until an operator
  switches one endpoint): the mode filter would remove nothing, so the claim is
  `both_modes_candidates`, the keyed claim's own query, unchanged. Its two index
  reads stop at `fetch_budget` rows whatever the number of endpoints.
- **Otherwise**: one query per claimable `hmac` endpoint, `endpoint_id = $1`, which
  `webhook_endpoint_due_idx (endpoint_id, next_attempt_at) WHERE state IN
  ('pending','failed')` (migration `0011`) answers from its first entry, in order,
  stopped by `LIMIT budget`. The per-endpoint lists are merged by `nextAttemptAt`,
  so the oldest-first order is the one the keyed claim has.

"In any state" is deliberate. An inactive or circuit-open `sealed` endpoint can
hold many due attempts: the keyed query would fetch the oldest of them and
`filter_by_endpoint_health` would drop them only afterwards, so they would fill
the over-fetch window and starve the `hmac` rows behind them. The fan-out leaves
them out before the window exists. The price of the rule is that one deactivated
`sealed` endpoint, kept for its history, keeps a deployment on the fan-out.

**The fan-out costs per endpoint, not per poll.** Each endpoint's query returns up
to `min(its due backlog, budget)` full rows (payloads included), so a mixed
deployment with `N` backlogged `hmac` endpoints reads and ships up to `N x budget`
rows a poll for the `budget` it keeps. Each is asked for `budget` and not
`fetch_budget`: unhealthy endpoints are left out before the queries, and no
endpoint can contribute more than `budget` rows to the oldest `budget` of the
merge, so the batch is the same unless an endpoint turns unhealthy mid-poll (an
under-filled poll, which `CANDIDATE_OVERFETCH_FACTOR` already allows). The first
version of this fan-out asked each for `fetch_budget` and cost three times that.

`EXPLAIN (ANALYZE, BUFFERS)` of the exact statements the claim sends (captured with
`log_statement = all`, parameters substituted), PostgreSQL 16.15, `budget` 20 so
`fetch_budget` 60, 400-byte payloads, attempts inserted in time order so that an
endpoint's due rows are spread over the heap as they are in production. `hmac`
endpoints are interleaved in time, `sealed` ones (20, with a tenth as many
`succeeded` rows as history) are due *before* every `hmac` attempt. Buffers are
`shared hit + read`, the time is the warm execution time summed over the
statements, and "rows" are `webhook_attempts` rows returned. "Before" is the
fan-out with `LIMIT fetch_budget` per endpoint (the claim as first reviewed),
"after" is this one:

| case                                                  | claim  | statements | attempt rows | buffers | time     |
| ----------------------------------------------------- | ------ | ---------: | -----------: | ------: | -------: |
| (a) all `hmac`: 50 endpoints x 2 000 due              | before |         52 |        3 000 |   3 189 | 12.09 ms |
| (a) all `hmac`: 50 endpoints x 2 000 due              | after  |          3 |           60 |      17 |  0.14 ms |
| (b) mixed: 200 000 `sealed`, 3 `hmac` x 5 due         | before |          6 |           15 |      31 |  0.44 ms |
| (b) mixed: 200 000 `sealed`, 3 `hmac` x 5 due         | after  |          6 |           15 |      20 |  0.27 ms |
| (c) mixed: 200 000 `sealed`, 50 `hmac` x 2 000 due    | before |         52 |        3 000 |   3 185 |  8.22 ms |
| (c) mixed: 200 000 `sealed`, 50 `hmac` x 2 000 due    | after  |         52 |        1 000 |   1 169 |  4.34 ms |

(a) is the keyed claim's own query, 60 rows however many endpoints there are; the
statements counted are the endpoints, that query, and the health re-read. (b) is the
same statements as before (three endpoints holding fewer than `budget` rows each;
the difference between its two rows is the table's state after the runs, not the
claim). (c) is what remains: still `N x budget` rows, a third of what it was, and
the reason a deployment with many backlogged `hmac` endpoints and one `sealed` one
should not be called cheap. What the fan-out removed is the sealed backlog from
the cost: the `EXISTS` query against it, `hmac` attempts due after 200 000 and
2 000 000 sealed ones, three `hmac` endpoints holding five due attempts between them:

| due sealed attempts | claim                        | buffers                           | time                  |
| ------------------- | ---------------------------- | --------------------------------- | --------------------- |
| 200 000             | before (one query, `EXISTS`) | 13 929                            | 101.6 ms              |
| 200 000             | fan-out (3 endpoint queries) | 4 + 4 + 4 (+ 1 for the endpoints) | 0.09 + 0.02 + 0.02 ms |
| 2 000 000           | before                       | 64 253                            | 877 ms                |
| 2 000 000           | fan-out                      | 4 + 4 + 4 (+ 1)                   | 0.10 + 0.05 + 0.03 ms |

The fan-out's cost does not move with the **sealed** backlog; it moves with the
number of `hmac` endpoints and with how much each holds, up to `budget` rows
apiece (4 buffers for an endpoint with a handful of due rows, 23 for one with more than
`budget`). The commit message of each round carries its figures.

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
(4 ms) at 200 000. It stays one `endpoint_id IN (...)` query, and it keeps the
lease index for itself; a second index on `(endpoint_id, lease_until)` would add a
write to every claim for a pile that cannot grow past the attempts in flight.

One cost is not removed: within an endpoint the index is ordered by
`nextAttemptAt`, and the `nextAttemptAt <= now` condition is a filter (it is
`IS NULL OR ...`), so an endpoint with a very long run of `failed` attempts
resting out a backoff is read to the end of that run on each poll. That is the
`hmac` work itself, and `webhook_due_idx` does the same.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
