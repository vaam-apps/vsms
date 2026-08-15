DLR ingestion and provider message-ref matching. #34.

Mounted as a raw route (`POST /dlr/{providerKey}`, §7 of the design
doc), not through `CrateStack`'s generated router — a provider webhook
carries no bearer token to validate against `GatewayAuth`, so it can't
go through the same auth path as every other route.
`backends/apps/sms-gateway`'s own `dlr.rs` owns that route and the provider-key
dispatch; this module owns matching an already-parsed
[`sms_provider::DeliveryUpdate`] against a `Message` and driving the
state machine from it.

# Scope

Landing a message in `delivered`/`uncertain`/`undelivered`/`failed`/
`expired` from a DLR is the whole of this module. What happens *after*
`undelivered` — `undelivered -> queued: retry` (§7.4) — used to be a
separate, not-yet-built concern (#122's own bug report: nothing drove
that edge, so a message that received exactly one retryable-failure DLR
sat in `undelivered` forever). `backends/crates/sms-worker/src/claim.rs`'s
`Claimable for Message::candidates()` now selects `undelivered` too, and
`Claimable::take_lease` drives it onward (retry via `-> queued`, or
straight to `-> failed` once `maxAttempts` is exhausted). This module's
own contribution to that fix is [`undelivered_retry_backoff`]: the write
below that lands a message in `undelivered` also stamps a backoff
`leaseUntil`, so `claim.rs`'s shared lease filter — the same mechanism
`routed -> queued`'s own backoff already relies on
(`dispatch::write_transition`) — holds the row back from being retried
immediately. A message whose `expiresAt` elapses before (or during) that
backoff is reaped to `expired` by `expire_stale`
(`backends/crates/sms-worker/src/jobs/expire_stale.rs`), not left to retry forever
either.
