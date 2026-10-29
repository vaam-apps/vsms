Assembling the generated router.

`router` takes `HmacWebhooks` (VSA-1), the answer `auth.methods` gives for
`hmac_webhooks`. It reaches the router twice: `Procedures::with_hmac_webhooks`,
and `enforce_hmac_gate`, the innermost layer, which refuses a generated
`WebhookEndpoint` write that would make an `hmac` endpoint. It sits under the RBAC
layers on purpose, so a caller who lacks `webhook:manage` is told so before being
told about the deployment's methods. `hmac_gate.md` has the reasoning.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
