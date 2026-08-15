The procedures the schema declares — eleven as of #50, not the seven
this doc comment used to claim (stale since #56/#57 added `requeueJob`/
`workerLocks` without correcting it; found while adding an eleventh,
`listMessageReceipts`, and fixed in the same edit rather than left to
drift further).

`previewMessage`, `sendMessage`, `provisionAppClient`, (#41)
`rotateWebhookSecret`, (#43) `replayWebhookAttempt`, (#56) `requeueJob`,
(#57) `workerLocks`, and (#50) `listMessageReceipts` are implemented.
`cancelMessage` and `enqueueJob` touch the job queue or a mutation this
milestone doesn't build yet; each returns a clearly-labelled error
naming the milestone that will build it, rather than a plausible-
looking stub that would pass a smoke test and lie.

# #71: `send`'s own span in the correlation chain

The framework's own generated `invoke_with_db` wrapper
(`cratestack-macros`'s `instrument.rs`) already logs
`cratestack_procedure = "sendMessage"` / `cratestack_request_id` /
`cratestack_duration_ms` around this whole call — that is the
HTTP-request-scoped half of #71's tracing requirement, and needs no
code here to exist. What that wrapper cannot log, because it runs
before and after `send` without seeing inside it, is the one value that
actually survives past this process: `Message.id`. [`Procedures::send`]
emits its own `info!` immediately after `create()` returns, carrying
`message_id` alongside `cratestack_request_id` (read directly off `ctx`)
— the join key `backends/crates/sms-worker/src/dispatch.rs`'s own submit-success
event and `backends/crates/sms-api/src/dlr.rs`'s own ingestion event reuse later,
in different processes, with no span context to inherit it through. See
`docs/runbooks/alerting.md`'s "Correlating a message end to end" section
for why `Message.id` is the join key and not a `traceparent`, and for a
worked example query across all three log lines.
