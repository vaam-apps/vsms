Assembling the generated router.

`router` takes `ApiAuth`, the answer `auth.methods` gives for the API: which doors
are open (`ApiDoors`, VSA-3b: the signed envelope, a machine token, or both; see
`bearer.md`) and `HmacWebhooks` (VSA-1), whether `hmac_webhooks` is permitted. The
doors set the envelope's mode and add `enforce_bearer_door` between it and the
per-principal limiter, and `keypair_gate.md` is the innermost layer's. `HmacWebhooks`
reaches the router twice: `Procedures::new`'s argument,
and `enforce_hmac_gate`, the innermost layer, which refuses a generated
`WebhookEndpoint` write that would make an `hmac` endpoint. It sits under the RBAC
layers on purpose, so a caller who lacks `webhook:manage` is told so before being
told about the deployment's methods. `hmac_gate.md` has the reasoning.

Five independent `enforce_route_permission` layers gate generated routes by permission,
one slice each (`PROVIDER_WRITE_ROUTES`, `JOB_READ_ROUTES`, `PROVIDER_ROUTE_READ_ROUTES`,
`SENDER_AND_WEBHOOK_WRITE_ROUTES`, `MESSAGE_READ_ROUTES`), for the reason `rbac.md` gives.
`MESSAGE_READ_ROUTES` asks `sms:read` of the message, receipt and part reads, because
`Message`'s own policy admits any person and a model with a `message` relation returns the
row with `?include=message`. A unit test reads the schema and fails when a model gains such
a relation with no gate and no named exception.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
