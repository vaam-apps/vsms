`SmsProvider` adapter for **AWS End User Messaging SMS** (the
`pinpoint-sms-voice-v2` API, `SendTextMessage`), with delivery receipts
delivered by **Amazon SNS** to `POST /dlr/aws_sms`.

## Why End User Messaging and not SNS `Publish`

SNS can send an SMS directly (`Publish` with a `PhoneNumber`), but it has no
way to report what happened to it except `CloudWatch` Logs. A message sent
that way would sit in `submitted` until `expire_stale` marked it `expired`,
firing a `message.expired` webhook for an OTP that arrived. End User
Messaging is the same SMS backend (the same sandbox, spend limits, sender
IDs and country coverage) with an API that can publish per-message events
to an SNS topic, which SNS then posts, signed, to this gateway. So SNS is
still in the picture; it carries the receipts.

## One provider, two halves

- **The worker sends.** [`AwsSmsProvider::sender`] needs the region, an IAM
  access key allowed `sms-voice:SendTextMessage`, the configuration set
  whose event destination is the SNS topic, and the account's commercial
  terms (`tps_ceiling`, `cost_per_segment_xaf`), none of which is defaulted.
- **The gateway receives.** [`AwsSmsProvider::receiver`] needs only the SNS
  topic ARN. It holds no AWS credentials, so a compromised gateway cannot
  send a single SMS on the account.

Each half refuses the other's job: a receiver's `submit` is `Unavailable`,
and a sender refuses every callback.

## Submit

One signed JSON 1.0 `POST /` (`X-Amz-Target:
PinpointSMSVoiceV2.SendTextMessage`), signed with the hand-written `SigV4` in
`sigv4` — no AWS SDK, so no `aws-lc-rs` (see `sigv4.md`). The request and
error shapes are botocore's service model, not guesses.

`Context` carries `vsmsMessageId` = our `Message.id`; AWS echoes it in every
event, so a raw receipt can always be tied to a message by a human.
Correlation itself uses the `MessageId` AWS returns, stored as
`providerMessageRef` and matched against each event's `messageId`.

| response | `ProviderError` | routing consequence |
|---|---|---|
| `200` + `MessageId` | — (`SubmitAck`) | |
| `200` without one, or unreadable | `Indeterminate` | `uncertain`, never resubmitted |
| connect failure | `Unavailable` | fail over; nothing was sent |
| timeout or body error after connecting | `Indeterminate` | AWS may have accepted it |
| `ThrottlingException` | `Transient` (1s) | retry here |
| `ServiceQuotaExceededException` (spend limit), `AccessDeniedException`, `ResourceNotFoundException` (no such configuration set or identity), any credential or signature error | `Unavailable` | open the breaker, fail over |
| `ValidationException` about the country, sender identity or price | `Permanent` | try the next route |
| any other `ValidationException`, `ConflictException` (opted out) | `Rejected` | fail the message |
| `5xx` | `Unavailable` | |

## Known limits

- **Long-lived IAM user keys only.** No instance-profile, IRSA or STS
  session credentials: those expire, and refreshing them is the AWS SDK's
  credential chain, which is the dependency this crate avoids. Scope the
  user to `sms-voice:SendTextMessage` on the one configuration set and
  origination identity, and rotate the key.
- **An `Indeterminate` submit is not rescued by a later receipt.** Its
  `MessageId` was never learned, so the event that eventually arrives
  matches nothing and `expire_stale` resolves the message — the same
  documented trade-off as Orange's (`AGENTS.md`, #119).
- **A final `TEXT_SUCCESSFUL` counts as `Delivered`.** See `events.md`.
- **No `delivering_network`.** Events carry `mcc`/`mnc`, but mapping those
  onto this schema's operator codes is not done here.
