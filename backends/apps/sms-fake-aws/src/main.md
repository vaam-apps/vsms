`sms-fake-aws`: [`sms_fake_aws::FakeAws`] as a long-lived process, for
`just demo` (`compose.dev.yaml`). **Not a production binary**: it impersonates
AWS, sends nothing to any handset, and accepts a fixed, public demo
credential by default.

For the gateway to accept its receipts, the gateway must be built with the
`fake-aws` feature and started with `AWS_SMS_FAKE_SNS_CERT_BASE` pointing
here; a release image has neither. See `docs/runbooks/aws-sms.adoc`'s "Without
an AWS account" section.
