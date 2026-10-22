The fake's own check of AWS Signature Version 4 on `SendTextMessage`.

Written again rather than reusing `sms_provider_aws_sms`'s signer, and held
to AWS's published `get-vanilla` test vector on its own. A verifier that
called the adapter's signing code would agree with any mistake in it; this
one reconstructs the canonical request from what actually arrived on the
socket (the received `Host`, the received body), so it also catches the one
class of bug a signer's unit test cannot: the signed request and the sent
request differing, as when an endpoint override's port or path is signed one
way and sent another.

Refusals use the error types AWS answers with, all `403`:
`MissingAuthenticationTokenException`, `IncompleteSignatureException`,
`UnrecognizedClientException` (another access key) and
`InvalidSignatureException` (wrong signature, wrong scope, or `X-Amz-Date`
more than 15 minutes from the fake's clock). The adapter classifies every one
of them as `Unavailable`, the same as against AWS.

AWS's documentation requires `host` and `x-amz-date` to be signed. The fake
also requires `x-amz-target`, which the adapter signs: a signature that left
it out would let a captured request be replayed as another operation.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
