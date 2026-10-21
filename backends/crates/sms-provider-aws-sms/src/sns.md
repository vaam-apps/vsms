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
   credentials, ending in `.pem`. Only after this does anything leave the
   process, so a forged message cannot make the gateway fetch an arbitrary
   URL.
4. **The signature verifies** against the certificate's public key, over the
   exact `name\nvalue\n` string SNS signed.

The certificate is trusted because it was fetched over HTTPS from the SNS
host for that region (reqwest validates that host's TLS certificate against
the web PKI, and redirects are disabled). The certificate's own chain is not
walked separately; that is the same trust model the AWS SDKs' message
validators use. Certificates are cached by URL.

`Timestamp` is not checked for freshness. SNS retries a failed delivery for
up to hours, and replaying a genuine receipt is harmless: ingestion is
idempotent, and an out-of-order transition is refused by the state machine.

## What a refusal and a failure each mean

A refusal is proof the message is not one we accept, so the gateway answers
`403` and SNS will not keep retrying a forger's message. A failure to *run*
the check (the certificate could not be fetched) is not proof of anything,
so it is an `Err`, the gateway answers `503`, and SNS redelivers it later.
Answering `403` there would throw away a genuine receipt because of a
transient network error.

## Subscription confirmation

After `aws sns subscribe … --protocol https`, SNS posts a
`SubscriptionConfirmation`. Once it passes every check above, visiting its
`SubscribeURL` (validated against the same host rule) confirms the
subscription. Doing this automatically is safe because of check 1: only a
confirmation for the configured topic is ever visited.
