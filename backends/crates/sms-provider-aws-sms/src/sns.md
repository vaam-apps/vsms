Authenticating the Amazon SNS deliveries that carry End User Messaging SMS
delivery events to `POST /dlr/aws_sms`.

That route is public and unauthenticated by design (a provider webhook
carries no bearer token), so without this module anyone who can reach it
could mark any message `delivered`. SNS signs every HTTP(S) delivery; this
module checks the signature, and refuses everything it cannot check.

Sources: the message formats are
<https://docs.aws.amazon.com/sns/latest/dg/http-subscription-confirmation-json.html>
and its sibling pages; the string to sign is transcribed from AWS's own Java
example (<https://docs.aws.amazon.com/sns/latest/dg/sns-example-code-endpoint-java-servlet.html>);
the checks follow
<https://docs.aws.amazon.com/sns/latest/dg/sns-verify-signature-of-message.html>.

## What is checked, in order

1. **`TopicArn` equals the configured events topic.** This is the check that
   actually matters, and the one most implementations forget: a valid SNS
   signature proves the message came *from SNS*, not from *our* topic.
   Anyone can create a topic in their own AWS account, subscribe this URL to
   it, and publish perfectly signed notifications. AWS's own guidance says
   to "reject any message with an unexpected `TopicArn`".
2. **`SignatureVersion` is `2`** (`SHA256withRSA`). Version 1 is `SHA1withRSA`.
   It is refused rather than verified, and the operator sets the topic's
   `SignatureVersion` attribute to `2` once (the runbook says how). Besides
   SHA1's own weakness, `rustls-webpki`, the verifier used here, does not
   implement SHA1 signatures at all.
3. **`SigningCertURL` is `https` on exactly `sns.<region>.amazonaws.com`**,
   where `<region>` is the configured topic's, with no port and no
   credentials, and its path is exactly
   `/SimpleNotificationService-<32 hex digits>.pem`, with no query and no
   fragment. Only after this does anything leave the process. A forged
   message therefore cannot make the gateway fetch an arbitrary URL: the
   host and the path shape are fixed, and the forger chooses only the 128
   bits of the file name. Those fetches are also rate limited (see below).
4. **The signature verifies** against the certificate's public key, over the
   exact `name\nvalue\n` string SNS signed.

The certificate is trusted because it was fetched over HTTPS from the SNS
host for that region (reqwest validates that host's TLS certificate against
the web PKI, and redirects are disabled). The certificate's own chain is not
walked separately; that is the same trust model the AWS SDKs' message
validators use. Certificates are cached by URL path (the host is always the
topic's region's), and only a certificate that was fetched and parsed is
cached.

## What a forged message can cost

Nothing is fetched until checks 1 to 3 pass, so what a forger can cause is
bounded: at most one GET per 5 seconds, process-wide, of an
`https://sns.<region>.amazonaws.com/SimpleNotificationService-<32 hex>.pem`
URL, whose body is read in chunks and abandoned past 64 KiB. Because only
successful fetches are cached and the path shape is fixed, a forger cannot
churn the cache either (the small cache cap is unreachable in practice).

The price of the rate limit is that a genuine message with a cold cache
(start-up, or a rotated SNS certificate) that arrives within 5 seconds of
any other uncached fetch gets a `503` and relies on SNS retrying; see the
next section for how much retrying that is.

`Timestamp` is not checked for freshness. Replaying a genuine receipt is
harmless: ingestion is idempotent, and an out-of-order transition is refused
by the state machine.

## What a refusal and a failure each mean, and what SNS does about it

A refusal is proof the message is not one we accept, so the gateway answers
`403`. A failure to *run* the check (the certificate could not be fetched,
or the fetch was rate limited) is not proof of anything, so it is an `Err`
and the gateway answers `503`.

That distinction matters because of how SNS treats the two. AWS's default
HTTP/S delivery policy is `numRetries` 3, with `minDelayTarget` and
`maxDelayTarget` both 20 seconds: three retries, 20 seconds apart. It
retries only `5xx` and `429`; every other response is a permanent failure
and is never retried
(<https://docs.aws.amazon.com/sns/latest/dg/sns-message-delivery-retries.html>,
the delivery policy table and the note on retryable errors). So:

- A `503` is redelivered for about a minute and then dropped. A gateway
  outage longer than that loses the receipts that arrived during it, and
  `expire_stale` resolves those messages. To ride out longer outages, give
  the subscription a custom HTTP/S delivery policy with a larger
  `healthyRetryPolicy` (the same page documents the fields).
- A `403` is final. Answering it for a genuine receipt would throw it away
  for good, which is why a transient fetch failure is never one.
- The one refusal that discards genuine receipts is a `SignatureVersion`
  other than `2` on our own topic. It is logged at `error`, because it is an
  operator misconfiguration that silently drops every receipt until the
  topic's `SignatureVersion` attribute is set to `2`. A foreign `TopicArn`
  is just a refusal: that is what forgery looks like.

## Subscription confirmation

After `aws sns subscribe … --protocol https`, SNS posts a
`SubscriptionConfirmation`. Once it passes every check above, the gateway
does **not** visit its `SubscribeURL`. It acknowledges the message and logs
at `warn`, with the `topic_arn` and the `token`, the exact command to run:

```sh
aws sns confirm-subscription --topic-arn <arn> --token <token> \
    --authenticate-on-unsubscribe true
```

The reason is `AuthenticateOnUnsubscribe`. Confirming by a plain GET of the
`SubscribeURL` leaves it `false`, and then anyone holding a notification's
`UnsubscribeURL` can remove the subscription. That URL is stored verbatim in
`DeliveryReceipt.rawPayload`, which any signed-in console role can read.
Confirming with the CLI and `--authenticate-on-unsubscribe true` makes SNS
require the subscriber's AWS credentials to unsubscribe.

The `SubscribeURL` is still checked to be on the topic's own SNS host, so a
URL SNS could not have written is a refusal rather than a log line.

An `UnsubscribeConfirmation` is acknowledged and logged at `error`: the
subscription is gone, so receipts stop until it is re-created.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
