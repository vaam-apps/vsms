The worker as a library. One binary, `sms-worker`, runs one or more
[`Role`]s selected at startup — see §7.1 of the design doc.

[`Role`]/[`run`] are #27's shape — a role-selectable binary — over
[`lease`]'s advisory-lock leader election (#28) and [`claim`]'s CAS claim
loop (#29). `Dispatch`'s real body ([`dispatch`], #33) is the first role
to actually call into [`claim::claim_batch`]; `Jobs`/`Scheduler`
([`jobs`]/[`scheduler`], #35) are the second and third; `Drain`
([`drain`], #39) is the fourth; `Hooks` ([`hooks`], #40) is the fifth.
`Smpp` is the only role still [`run`]'s idle stub, and stays that way
until M7.

This crate depends on `cratestack` (for [`lease`]'s raw-`sqlx` R1
exception) and, since #29, `sms-api` (for the expanded schema
[`claim`] claims against) — `include_server_schema!` is still invoked
exactly once, in `sms-api`'s own `lib.rs`; linking its already-compiled
output here doesn't re-run that expansion. Since #33, also `sms-provider`
(for [`WorkerContext`]'s provider registry — this crate holds the
trait, never a concrete adapter, the same way `sms-api` never does).
Since #62, also `sms-msisdn` and `sms-routing` ([`routing`]'s own I/O
glue over the pure route-selection engine).
