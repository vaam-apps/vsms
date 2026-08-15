`expire_stale` — the one real `kind` this milestone wires up. §7.5's
own table: "`submitted`/`uncertain` past validity → `expired`", 1-minute
cadence. #122 added a third rule for `undelivered`, on the same
`expiresAt` clock as `submitted` — see below.

Three separate rules, not one, because the states measure "past
validity" against different clocks:

- `submitted -> expired: no DLR in window` (§7.4) uses `Message.expiresAt`
  directly — the same validity budget set at creation (15 min for `otp`,
  24h for `notification`), unconsumed by the time a DLR should have
  arrived.
- `uncertain -> expired: 6h timer` (§7.4) is a *fresh* clock, not tied to
  the original `expiresAt` — a message can turn `uncertain` well within
  its original window, and per §7.4 "never retried automatically", it
  gets its own 6-hour grace regardless of how much of the original
  window was left. The schema has no dedicated
  `enteredUncertainAt` field, so this uses `updatedAt` (bumped by the
  `Timestamps` mixin's own touch trigger on every write) as the proxy
  for "when it became `uncertain`" — correct as long as nothing else
  writes to an `uncertain` message before this job or a late DLR does,
  which holds: `uncertain` is not itself a target of any operator or
  retry action in §7.4's diagram, only DLRs and this job ever move it.
- `undelivered -> expired` (§7.4, #122) uses `Message.expiresAt` directly,
  the same clock as `submitted`, deliberately *not* a fresh grace period
  like `uncertain`'s: `undelivered` is a genuinely retryable failure —
  `backends/crates/sms-worker/src/claim.rs`'s `Claimable for Message` now selects
  it and retries via `-> queued` — but a retry that would only deliver
  after the message's own original validity budget is gone is exactly
  the outcome `expiresAt` exists to prevent (§7.4's own backoff
  paragraph: "capped by `maxAttempts` and hard-stopped by `expiresAt`").
  `claim.rs`'s shared `expiresAt` filter already stops retrying such a
  row; this rule is what actually moves it off `undelivered` once that
  happens, since retry exhaustion alone (`maxAttempts`) already reaches
  `failed` on its own via `claim.rs`'s own `undelivered` branch and
  never needs this job at all — this rule exists for the row that's
  still under `maxAttempts` but has simply run out of time.
