The fake SNS topic's signing identity.

An RSA key is generated at start-up and wrapped in a self-signed certificate,
so no private key is ever committed and no two runs share one. The receiver
verifies a notification with its real code path: it fetches the certificate,
parses it with `webpki`, and checks `RSA PKCS#1 v1.5 SHA-256` over the
string-to-sign. Nothing about the receiver is relaxed except *where* the
certificate is fetched from, and only when the caller opts in (see
`RedirectSnsHttp` in the crate root).

The file name is `SimpleNotificationService-<first 16 bytes of the
certificate's SHA-256, hex>.pem`. AWS's own names have the same shape, which
is all the receiver checks.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
