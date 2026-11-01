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

**A number reaches only unpurged rows inside the last 90 days.** `purge_retention`
keeps `msisdnHash` and the rows are kept forever, and the keyed hash is what
de-identifies a purged row: a number that matched one would link a plain number to
purged history in every app. So the filter carries `purgedAt IS NULL` (a row purged
early, an erasure, is not found by number whatever its age), and the window of a
lookup by number may not start before now minus [`MAX_WINDOW_DAYS`], a `Validation`
with a fixed message. Capping the window's width alone would not hold it, since
`until` can be any past date. An id still answers for a purged message
(`recipient` null, `purged` true); a client reference matches nothing purged, since
the purge nulls it. No `Validation` message quotes what was typed: an invalid number
is one fixed sentence.

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
period); for a number it may not start more than 90 days before now either. A
lookup by `messageId` is unbounded unless the caller names a window. `limit`
defaults to 10, at most 50, newest first; one more row is read than asked for, to
say `truncated`.

# The audit row, and the budget

Every lookup that reaches the database writes one `DeliveryLookup` row **before it
answers**, under the same system context, and **the call fails if a write fails**.
The row's `keyValue` is a message id, or a keyed hash of the number or of the client
reference; never the thing typed. Its `outcome` is `pending`, `answered` (with the
`resultCount`) or `over_budget`.

What a caller can guess is budgeted, [`BUDGETED_LOOKUPS_PER_HOUR`] (60) an hour: a
lookup by number, and a **person's** lookup by client reference (references are
guessable, and a person reaches every app). A message id is not counted, nor is an
app key's lookup of its own client reference. The count is keyed on the caller's
subject **and** its kind, so an app key and a person that share a subject string do
not share an hour.

A budgeted lookup inserts its `pending` row first and counts after, its own row
included, and is admitted only if the count is at most the budget. Every write
autocommits, so when an admitted row counts, every row admitted before it is
visible and still counted, and at most 60 are admitted, however many arrive
together, with no lock. Counting first and inserting
after lets a parallel burst all count before any inserts. The price is that a burst
larger than the budget can be refused more than it must be: a row another call
counted may be marked `over_budget` a moment later. A refused attempt is marked
`over_budget` (the 61st is a `Forbidden`) and is not counted again; an admitted one
is marked `answered` with its count. Those are the only updates a row takes, and
`delivery_lookups_update_guard` refuses any other. A lookup that is not budgeted is
written once, as `answered`.

The console's Delivery lookup page reads these refusals by their words: the
`delivery lookup budget exhausted` prefix of the 403 and the fixed `Validation`
messages for a number that is not a mobile number, a window wider than 90 days,
a number reaching back more than 90 days, and `since` after `until`. Rewording
any of them means updating `frontends/apps/admin/app/delivery-lookup/lookup-error.ts`
and its test in the same change.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
