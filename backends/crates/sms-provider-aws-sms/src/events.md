End User Messaging SMS event records, as published to the configuration
set's SNS event destination, mapped onto `DeliveryOutcome`.

The record format and status meanings are AWS's own
(<https://docs.aws.amazon.com/sms-voice/latest/userguide/configuration-sets-event-format.html>);
the list of event types is botocore's `pinpoint-sms-voice-v2` `EventType`
enum. `provider_ref` is the record's `messageId`, which is the same value
`SendTextMessage` returned and `submit` stored as `providerMessageRef`.

## Only a final event is a verdict

AWS sends `isFinal` on every record. Anything other than `TEXT_DELIVERED`
with `isFinal` not `true` (including a record that omits it) is `InFlight`:
progress, recorded without changing the message's state. A non-final failure
can still be followed by a delivery, and mapping it to a failure would let
the retry path resubmit a message that then arrives, which is a duplicate
SMS. `TEXT_DELIVERED` needs no finality; it is the best news there is.

## `TEXT_SUCCESSFUL` is `Delivered` only when it is final

`TEXT_SUCCESSFUL` means "accepted by the phone carrier". On its own that is
progress, the same rung Orange's `DeliveredToNetwork` sits on, and it maps
to `InFlight`. But many carriers never return a handset receipt to AWS, and
for those AWS sends `TEXT_SUCCESSFUL` with `isFinal: true` — its own example
record is exactly that. No later event will arrive. Mapping that to
`InFlight` would leave the message `submitted` until `expire_stale` marks it
`expired`, firing `message.expired` for traffic the carrier accepted.

So a final carrier acceptance is recorded as `Delivered`. That is a
deliberate overstatement: it means "the network took it and AWS will tell us
nothing more", not "the handset confirmed it". `DeliveryReceipt.rawStatus`
keeps `TEXT_SUCCESSFUL`, so the two are always distinguishable by anyone who
needs to.

## The rest

The outcomes below are for a final event; a non-final one is `InFlight`
whatever its type, except `TEXT_DELIVERED`.

| event type                                                                                                          | outcome     | why                                                                                                |
| ------------------------------------------------------------------------------------------------------------------- | ----------- | -------------------------------------------------------------------------------------------------- |
| `TEXT_DELIVERED`                                                                                                    | `Delivered` | accepted by the handset                                                                            |
| `TEXT_QUEUED`, `TEXT_PENDING`, `TEXT_SENT`                                                                          | `InFlight`  | progress; see `DeliveryOutcome::InFlight` for why this must not be `Uncertain`                     |
| `TEXT_UNREACHABLE`, `TEXT_CARRIER_UNREACHABLE`                                                                      | `Failed`    | AWS: "usually transient … try again later", so retryable (`undelivered`)                           |
| `TEXT_TTL_EXPIRED`                                                                                                  | `Expired`   | the `TimeToLive` (or AWS's 72 hours) ran out                                                       |
| `TEXT_INVALID`, `TEXT_INVALID_MESSAGE`, `TEXT_BLOCKED`, `TEXT_CARRIER_BLOCKED`, `TEXT_SPAM`, `TEXT_PROTECT_BLOCKED` | `Rejected`  | resending the same message to the same number fails the same way                                   |
| `TEXT_UNKNOWN`                                                                                                      | `Uncertain` | AWS's own "we do not know what happened", sent when no final carrier event arrived within 72 hours |
| any other `TEXT_*`                                                                                                  | `Unknown`   | never guessed                                                                                      |

Events that are not `TEXT_*` (voice, MMS, RCS on a shared configuration set)
produce no update and are acknowledged, so SNS does not redeliver them.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
