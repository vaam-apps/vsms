`lookupDeliveryStatus`: the body-free delivery-status lookup, gated on
`delivery:read`. `docs/architecture.md` section 5.2, "Implementation, delivery
lookups (delivery:read)", has the sequence and state diagrams and the decisions
behind the rules below.

# What it answers

"Did this message arrive?" for someone who may not read the message. The answer
is [`schema::DeliveryStatus`], a type of its own: the masked recipient, the
lifecycle timestamps, the state and the newest receipt. It carries no body, no
`bodyHash`, no `bodyLength`, no `msisdnHash`, no `costXaf`, no `stateReason` (free
text a provider's error put there), no `clientRef` or `idempotencyKey`, no
`providerMessageRef*` and no receipt `rawPayload`. A test reads `schemas/vsms.cstack`
for each of those names, so a field that creeps back fails the build.

# Three keys, exactly one per call

| Key         | Who                                                     | Scope                                      |
| ----------- | ------------------------------------------------------- | ------------------------------------------ |
| `messageId` | a person or an app key                                  | an app key sees its own app's message only |
| `clientRef` | a person (with `appId`) or an app key (own app implied) | one app                                    |
| `msisdn`    | a person or an app key                                  | the whole number, hashed under the pepper  |

A number is looked up by **equality on `Message.msisdnHash`**, never by prefix,
suffix or list: the procedure cannot be asked "which numbers have we messaged".
The answer shows the number masked even though the caller typed it whole. Like
`searchOptOutByMsisdn`, a rotated pepper finds nothing and the procedure cannot
say so.

# Who may call, and how the read is scoped

The first statement of the procedure's body is `require_permission(ctx,
"delivery:read")`. There is **no new `Principal` claim**: `Message`'s `@@allow`
asks for `messageRead` and must not learn a `deliveryRead`, because a claim there
would widen `GET /messages` to everyone who holds `delivery:read`. The read runs
under `system_context("sms-api:delivery-lookup")`, so the model's policy does not
decide it and the body does: an app key's every query carries `appId == its own`,
and an app key naming another app is refused (`Validation`), not ignored.

# The window

`since` and `until` bound `Message.createdAt`. For a `clientRef` or a number they
default to the last seven days, and a window may not exceed 90 days (the retention
period). A lookup by `messageId` is unbounded unless the caller names a window.
`limit` defaults to 10, at most 50, newest first; one more row is read than asked
for, to say `truncated`.

# The audit row, and the budget

Every answered call writes one `DeliveryLookup` row before it answers, under the
same system context, and **the call fails if the row cannot be written**. The row's
`keyValue` is a message id, or a keyed hash of the number or of the client
reference; never the thing typed. A caller's number lookups in the last hour are
counted from those rows, and the 61st is refused with `Forbidden`
([`MSISDN_LOOKUPS_PER_HOUR`]). Lookups by id or client reference are not counted.
The check and the insert are two statements, so concurrent calls from one caller can
overshoot by the number in flight at once.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
