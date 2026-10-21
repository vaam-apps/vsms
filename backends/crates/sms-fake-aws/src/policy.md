What the fake does with each `SendTextMessage` call.

A [`SubmitDecision`] is the HTTP answer, an optional delay before giving it,
and the delivery events to publish to the topic if the message was accepted.
The named constructors are the cases `sms-provider-aws-sms` classifies
differently, so a test can reach each branch of that classification through a
real HTTP exchange:

| constructor | what AWS says | what the adapter makes of it |
|---|---|---|
| `delivered_after` | `200`, then `TEXT_PENDING`, then `TEXT_DELIVERED` | `submitted`, then `delivered` |
| `accepted_with_events` | `200`, then the given events | depends on the events |
| `without_message_id` | `200 {}` | `Indeterminate`: `uncertain`, never resent |
| `throttled` | `ThrottlingException` | `Transient`: retried on the same route |
| `sandbox_unverified` | `ConflictException`, `DESTINATION_PHONE_NUMBER_NOT_VERIFIED` | `Permanent`: fail over |
| `opted_out` | `ConflictException`, `DESTINATION_PHONE_NUMBER_OPTED_OUT` | `Rejected`: fail the message |
| `spend_limit_reached` | `ServiceQuotaExceededException` | `Unavailable`: fail over, count toward the breaker |

`delayed` holds the answer after recording the message. Longer than the
adapter's request timeout, that is a post-accept timeout: AWS has the
message and the caller does not know it.

A [`FaultPolicy::Scripted`] queue that runs out answers
`500 InternalServerException`. `sms-fake-orange`'s scripted policy degrades to
a bare accept instead, which suits nothing long-lived; here a test that
submits more often than it scripted fails visibly.
