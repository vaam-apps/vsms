# SNS signature fixtures

Test-only. Nothing here is an AWS certificate or an AWS-signed message.

- `test-sns-signing-cert.pem` — a self-signed RSA-2048 certificate
  (`CN=TEST FIXTURE - not an AWS certificate`). Its private key was used once
  to sign the two JSON files below and then **deleted**; it is not in this
  repository, so nothing can sign new messages with it.
- `sns-notification-delivered.json` — an SNS `Notification` whose `Message`
  is an End User Messaging `TEXT_DELIVERED` event.
- `sns-subscription-confirmation.json` — an SNS `SubscriptionConfirmation`.

Both are `SignatureVersion` 2 (SHA256withRSA). The string to sign was built by
a separate Python implementation following AWS's own Java example
(<https://docs.aws.amazon.com/sns/latest/dg/sns-example-code-endpoint-java-servlet.html>)
and signed with `openssl dgst -sha256 -sign`, so the Rust verifier is checked
against a third tool rather than against its own canonicalisation. Each was
then re-verified with `openssl dgst -sha256 -verify` before being committed.

To change a fixture, generate a fresh key and certificate
(`openssl req -x509 -newkey rsa:2048 -nodes …`), re-sign both files the same
way, replace the certificate, and delete the new key again.
