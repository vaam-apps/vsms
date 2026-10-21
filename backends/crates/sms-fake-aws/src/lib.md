A fake of AWS End User Messaging SMS and of the Amazon SNS topic that
carries its delivery events, standing in for AWS the way `sms-fake-orange`
stands in for Orange.

It plays both of AWS's parts in `sms-provider-aws-sms`'s lifecycle:

- **`SendTextMessage`** (`POST /`): checks the request's Signature Version 4 signature
  independently (`sigv4.md`), requires the configuration set it was told
  exists, records the request in a [`Ledger`], and answers per a
  [`FaultPolicy`] (accept, throttle, the sandbox refusal, a missing
  `MessageId`, and so on).
- **The events topic**: for an accepted message it publishes the
  [`EventStep`]s of its decision as SNS `Notification`s, signed with a key
  made at start-up (`signing.md`), posted to the configured DLR endpoint and
  retried until acknowledged (see [`Redelivery`]). The certificate is served
  at `GET /SimpleNotificationService-<hex>.pem`.

The receiver trusts the fake only if it is told to: notifications name a
`SigningCertURL` on AWS's real SNS host, and the receiver's checks apply
unchanged. [`RedirectSnsHttp`] is the one seam: it fetches that URL's path
from the fake instead of from AWS. A live test builds the receiver with it
directly; `sms-gateway` reaches it only through its off-by-default
`fake-aws` cargo feature, which no release image is built with.

## What it proves, and what it does not

The adapter and the fake were both written from the same reading of AWS's
documentation (botocore's `pinpoint-sms-voice-v2` model, AWS's event-record
and SNS signing docs). So a green end-to-end run proves the code is
consistent with that reading, over real HTTP, through the real signature
code on both sides. It does **not** prove AWS behaves that way; only a real
account does. Where the fake's checks are independent of the adapter's
code, they are held to AWS's published vectors rather than to the adapter
(Signature Version 4's `get-vanilla`).

Not modelled: carriers, real delivery times, prices, the sandbox's verified
numbers (a decision decides), opt-out lists, spend accounting, and
`UnsubscribeURL` (present, never served).
