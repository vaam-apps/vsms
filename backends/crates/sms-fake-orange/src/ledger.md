The request ledger — every submit [`FakeOrange`](crate::FakeOrange)
received, queryable by test code, plus a counter of DLR-delivery tasks
still in flight so a test can wait for the fake's own background work to
settle instead of guessing a sleep duration.

This is how a chaos test detects double-submission *from the provider's
own side* — "did Orange receive this reference twice?" — rather than
only inferring it from our database's own `attempts` column, which
cannot distinguish "one HTTP call that was retried at the transport
level" from "two logically separate submit attempts".
