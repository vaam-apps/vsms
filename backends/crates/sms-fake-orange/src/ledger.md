The request ledger — every submit [`FakeOrange`](crate::FakeOrange)
received, queryable by test code, plus a counter of DLR-delivery tasks
still in flight so a test can wait for the fake's own background work to
settle instead of guessing a sleep duration.

This is how a chaos test detects double-submission *from the provider's
own side* — "did Orange receive a submission for this recipient twice?"
— rather than only inferring it from our database's own `attempts`
column, which cannot distinguish "one HTTP call that was retried at the
transport level" from "two logically separate submit attempts".

# Why `SubmitRecord` correlates by `to`, not by a caller-supplied reference

Every record used to carry `reference` — `receiptRequest.callbackData`
from the wire, always `Message.id` in practice, since
`OrangeCmProvider::submit` set it to the caller's own reference on every
call. That field is gone: Orange's real, documented submit request
(<https://developer.orange.com/apis/sms/getting-started>) carries no
caller-supplied correlation token at all, so `OutboundSmsMessageBody`
doesn't send one any more, and this fake — mirroring that reality rather
than a convenient fiction — has nothing on the wire to extract it from
either.

`SubmitRecord::to` (the destination address, real and always present on
the wire) is what's left for a test to group by. It's a proxy, not a
promise: two *different* messages sent to the same recipient in the same
test would be indistinguishable by this field, exactly as they would be
to real Orange. Tests that need per-message correlation across several
concurrent messages (the seeded chaos sweep in
`backends/crates/sms-worker/tests/chaos_live_postgres.rs`) seed a
distinct recipient per message for exactly this reason.

`SubmitRecord::resource_id` is the *other* half — the id this fake mints
itself, mirroring what real Orange would mint, echoed back in both the
submit response and (if scheduled) the DLR. It's what actually drives
production-shaped correlation (`Message.providerMessageRef`); it is
deliberately *not* useful for "how many times was this message
resubmitted," since a fresh one is minted every call, matching the same
"Orange can't recognise a retry as a retry" reality `to` exists to work
around from the test-harness side.
