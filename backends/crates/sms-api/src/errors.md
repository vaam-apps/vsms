Translating database-level rejections into HTTP-shaped errors.

R2 says state transitions are proposed by Rust and decided by Postgres. The
deciding half is a `BEFORE UPDATE` trigger that raises SQLSTATE `SM001` on
an illegal edge. Left untranslated that arrives as
[`CoolError::DatabaseTyped`], which the framework maps to
`500 DATABASE_ERROR` — and a 500 reads as "the gateway is broken" when the
truth is "you asked for a transition that does not exist". Callers retry
500s and do not retry 409s, so the distinction changes their behaviour, not
just their logs.

# #71: this is also the one metrics choke point

[`map_database_error`] is the single place this workspace already
translates a raw `SM001` into something a caller can act on, so it is
also where [`sms_metrics::record_sm001`] is called — every SM001 either
process ever sees, from a generated CRUD write, a procedure (this
crate's own `replayWebhookAttempt`), or `sms-worker`'s claim loop and
per-role write sites (which route their own write errors through this
same function before logging or branching — see `backends/crates/sms-worker/src/
claim.rs`, `dispatch.rs`, `jobs.rs`, `hooks.rs`, and `jobs/
expire_stale.rs`, plus this crate's own `dlr.rs`), passes through here
exactly once. §9.1 of the design doc calls the resulting metric "the
highest-signal one in the list... in a correct system it is flat zero."
