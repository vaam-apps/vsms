Mounting the raw DLR webhook route. #34, extended for a second adapter (MTN,
issue #61) and for a third, AWS, with the callback-authentication hook.

`POST /dlr/{providerKey}` — not CrateStack-routed (see
`sms_api::dlr`'s own module doc for why: a provider webhook carries no
bearer token to validate against `GatewayAuth`). The path segment must match
one of the configured providers' own `key()`; that is the first access
control. What happens next depends on whether the provider documents a way
to prove a callback is its own:

- **`aws_sms` authenticates every call.** Amazon SNS signs each delivery, and
  `AwsSmsProvider` checks the `TopicArn`, the signature version, the
  signing-certificate URL and the RSA-SHA256 signature before anything is
  parsed (`sms-provider-aws-sms`'s own `sns.md`). This is the only adapter
  that does.
- **`orange_cm` and `mtn_cm` do not.** Neither provider documents a callback
  signature scheme, so there is nothing to check: their `authenticate_callback`
  is the trait default, `Ingest`. For Orange the only controls are §9.2's
  external constraint (it will only call a webhook on HTTPS 443 with a
  CA-signed cert, whitelisted per a manual support ticket) and the path
  segment. `RawCallback` carries the exact, unmodified bytes a future scheme
  would need. Orange's DLR shape was transcribed from Orange's own published
  documentation but has never been received from a live account (see
  `sms-provider-orange-cm`'s own `dlr` module); MTN's is transcribed from
  MTN's vendored MADAPI Swagger and is likewise unverified against a live
  account (`sms-provider-mtn`'s own module doc has the honesty ledger).

`{providerKey}` is looked up in a map keyed by each configured adapter's
own `SmsProvider::key()` (`DlrProvider`, this module), not compared
against a single hardcoded provider. Up to three adapters can be configured
(`orange_cm`, `mtn_cm`, `aws_sms`; see `commands::serve::build_dlr_router`),
each optional as long as at least one is. A 404 means "no adapter is
configured under this key", and that is the only thing it can mean. For
`aws_sms` the gateway builds a receiver from `AWS_SMS_EVENTS_TOPIC_ARN`
alone and holds no AWS credentials. If that variable is set but no
`Provider` row keyed `aws_sms` exists, the gateway refuses to start, the same
way it does for the other providers; seed the row first
(`docs/runbooks/aws-sms.adoc`).

## Authenticate, then parse

Before `parse_dlr` sees a body, the handler calls
`SmsProvider::authenticate_callback` (an async step because SNS verification
fetches a certificate; `parse_dlr` stays synchronous and pure). Its result
decides the response:

| `authenticate_callback` | response                        | why                                                                                                                                                                                            |
| ----------------------- | ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Ok(Refused)`           | `403`, nothing parsed or stored | The callback failed authentication. The reason is logged at `warn` and never returned to the caller.                                                                                           |
| `Ok(Acknowledged)`      | `200`, nothing parsed           | An authentic subscription handshake with no delivery update.                                                                                                                                   |
| `Err(_)`                | `503`                           | The check could not run (a certificate could not be fetched, or the fetch was rate-limited). That is not proof of forgery, so SNS is asked to redeliver instead of dropping a genuine receipt. |
| `Ok(Ingest)`            | the ingestion flow below        | Authentic, or the provider has no scheme to check.                                                                                                                                             |

A refusal is final for the sender. SNS treats any status other than `5xx` or
`429` as a permanent failure and stops retrying, so a genuine receipt that is
wrongly refused (for example, a topic still on `SignatureVersion` 1) is lost.
The `warn`, and for a version-1 topic an `error`, in the gateway log is the
only record.

The response status on a successful ingest is `200 OK`, not `202
Accepted` — Orange's own published contract
(<https://developer.orange.com/apis/sms/getting-started>, §4) states the
webhook receiver "must return an HTTP 200 OK in order to acknowledge the
receipt" and shows exactly that as its worked example. `docs/architecture.md`
§3.2's "return 202, never 200" rule governs `sendMessage`, this API's own
*outbound* send endpoint (the reasoning is "you have not sent anything
yet" — not applicable to a webhook acknowledging something a provider
says already happened); §3.2/§4.1/§6.2 all state this distinction
explicitly and record that this exact rule was once mis-cited the other
way around to justify `202` on this very route.

Ingestion proposes a state transition only when it is legal from the
message's current state, for every provider. A receipt that arrives after the
message has already reached a terminal state (a late `delivered` after
`expired`, say) is still recorded as a `DeliveryReceipt` but changes nothing,
and no longer produces a `SM001` rejection that the alert would count. Errors
in a body that `parse_dlr` cannot read at all remain `400`.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
