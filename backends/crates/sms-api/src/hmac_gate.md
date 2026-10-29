`auth.methods` without `hmac_webhooks`: the part of the switch that lives in the
API (VSA-1, owner ruling R5).

vsms serves more than one application, and a deployment that wants only the
keypair says `auth: {methods: [keypair]}` (`sms-config`, `auth.md`). An `hmac`
webhook endpoint is the one place a shared secret is left in this system, so with
the method off nothing may create one, move one to `hmac`, or mint a secret.
[`HmacWebhooks`] is what the router and the procedures are given; the YAML is
read once, in `sms-gateway serve`, and nothing below it knows a file exists.

# Where it is enforced

| Call                                               | Refused when `hmac_webhooks` is off                                   | By                          |
| -------------------------------------------------- | --------------------------------------------------------------------- | --------------------------- |
| `POST /webhook_endpoints` (generated create)       | the body names `deliveryMode: hmac`                                   | [`enforce_hmac_gate`]       |
| `PATCH /webhook_endpoints/{id}` (generated update) | the body names `deliveryMode: hmac`, or sets `secret` or `prevSecret` | [`enforce_hmac_gate`]       |
| `switchWebhookDeliveryMode` to `hmac`              | always; to `sealed` is allowed                                        | `Procedures::switch_mode`   |
| `rotateWebhookSecret`                              | always                                                                | `Procedures::rotate_secret` |

Every refusal is the same: HTTP 422, the envelope's `code` `VALIDATION_ERROR`
(cratestack gives every 422 that one code), and a message that **starts with
`auth_method_disabled:`**, which is the stable word a client matches, as it
matches cratestack's own `idempotency_key_conflict`. A sealed create, an update
that touches neither the mode nor a secret (`active`, `eventTypes`, `url`, a
sealed audience), a switch to `sealed` and a `DELETE` still work, so an operator
who turned the method off can deactivate, retarget, convert or remove what it
left behind.

# Why the generated routes need a middleware

`switchWebhookDeliveryMode` and `rotateWebhookSecret` are procedures, and a
procedure can look at a flag in its own body. The generated `POST` and `PATCH`
are not ours: they decode a body into `CreateWebhookEndpointInput` and write it,
and (as `map_database_error`'s doc says) they do not pass their errors through
our mapping either. So the gate is a layer, [`enforce_hmac_gate`], built the way
`owner_grant` is: it decodes the body with the router's own codec into the
handler's own input type, so it reads what the handler would act on, and it
fails closed (an undecodable body is a 400, never forwarded). When the method is
permitted it is a no-op that does not even buffer the body.
`every_generated_webhook_endpoint_write_route_is_guarded` fails when the schema
grows a route under `/webhook_endpoints` that is neither guarded nor classified.

The CHECKs of `0010_sealed_webhooks` stay the backstop for every other writer
(`psql` included); a gate in the database would need the setting to live there,
and the setting is a deployment's, in YAML.

# The worker

The same flag reaches `sms-worker` (`WorkerContext::hmac_webhooks`): a delivery
to an `hmac` endpoint is **not claimed**: the attempt stays `pending`, untouched,
until the method is permitted again. `sms-worker`'s `hooks.md` has the semantics
and the reason each choice was made.

# Boot

[`HmacBoot`] counts the `hmac` endpoints at start and decides what to say about
them and how loudly; `serve` and the `hooks` worker both read it, so the two lines
are one sentence. Each process emits it itself: a log line's target is the crate
that emits it, and the worker's default filter, which the compose file and the chart
repeat, leaves `sms_api` out.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
