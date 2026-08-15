#54: the route simulator — "given this recipient, class and app, which
route wins and why" without sending anything.

# Expose the engine's `Decision`, never re-decide

`sms_routing::select_route` (`backends/crates/sms-routing`, #62) already computes
the whole answer, including the explanation trail — a caller-supplied
`draw` is what makes it deterministic and replayable (see its own doc).
Everything in this module is either **fetching** the rows the engine
needs (identical shape to `backends/crates/sms-worker/src/routing.rs::decide`) or
**rendering** the `Decision` it returns onto the wire (`decision_to_wire`)
— never a second implementation of matching. `the_wire_result_matches_the_engines_own_decision`
below is the guard that proves the rendering step can't silently drift
from what the engine actually decided; see its own doc for how it was
confirmed to actually fail before being trusted.

# Why this duplicates `sms-worker`'s own I/O glue

`backends/crates/sms-worker/src/routing.rs` already does exactly this fetch +
convert dance for production dispatch. This module can't call it: `sms-
worker` depends on `sms-api` (for `schema::Cratestack` and friends), so
the dependency can't run the other way without a cycle — confirmed by
`sms-worker.workspace = true` sitting in `backends/crates/sms-api/Cargo.toml`'s
own `[dev-dependencies]` (test-only, deliberately never `[dependencies]`,
per that file's own comment on `worker_locks_live_postgres.rs`). The root
`Cargo.toml`'s own comment on the `sms-routing` dependency edge names
this exact situation as expected: "a future #54 simulator procedure in
sms-api is expected to depend on this directly too" — `sms_routing`
itself (the pure engine) is shared; the I/O glue around it is not, and
is small enough (four straightforward `match`/field-copy functions) that
duplicating it is cheaper than inventing a third crate beneath both just
to share sixty lines. If a third caller of this exact glue ever shows up,
that calculus changes — matching the precedent `sms-provider-mtn`'s own
module doc already sets for `classify_transport_error`'s identical
two-crate duplication.
