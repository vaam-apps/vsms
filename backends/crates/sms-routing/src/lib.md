The routing rules engine — #62, §6.3 of the design doc.

Pure, like `sms-encoding`/`sms-msisdn`/`sms-provider`: no `cratestack`
dependency, no schema types, no I/O. [`select_route`] is a plain
function from (a caller-fetched set of `Route` rows, a caller-fetched
set of `Provider` availability, a candidate message's attributes, an
injected random draw) to a fully-explained [`Decision`] — nothing here
ever queries a database or calls an RNG itself.

That purity is not a style preference; it is the load-bearing property
#62 was opened to guarantee. The admin route simulator (#54) has to
answer "given this recipient, class and app, which route wins and why"
*without sending anything* — which means the selection logic has to be
callable with no side effects at all, and its answer has to be a data
structure a UI can render, not a log line. See [`select_route`]'s own
doc for exactly how the weighted-random half of §6.3 ("sort by priority
then weighted-random within a priority band") is reconciled with
"deterministic and explainable".

`backends/crates/sms-worker/src/routing.rs` is the I/O glue: it fetches
`Route`/`Provider` rows under a system context, converts the schema's
own `OperatorCode`/`MessageClass` enums onto this crate's mirrored ones
(the same pattern `dispatch.rs`'s `decode_encoding` already uses for
`Encoding`), draws the one random `f64` production needs, and applies
the resulting [`Decision`] to a `Message` row.
`backends/crates/sms-api/src/route_simulator.rs` (#54) depends on this
crate directly too, replaying the exact same [`select_route`] call with
a caller-supplied draw instead of a random one — a second, necessarily
separate copy of the fetch-and-convert glue above, since `sms-api`
cannot depend on `sms-worker` (the dependency runs the other way).
