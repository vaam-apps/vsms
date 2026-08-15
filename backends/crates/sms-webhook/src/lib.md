Outbound webhook signing (`#41`) — HMAC-SHA256 over a canonical string,
with a Unix timestamp folded into the signed bytes to bound replay to
event freshness. Pure, with one deliberate exception
([`generate_secret`], which reads OS randomness): no schema coupling,
no clock reads — every function that needs "now" takes it as a
parameter rather than calling one.

`backends/crates/sms-worker`'s `hooks` role (#40) is the intended sender-side
caller of [`sign_header`]; `backends/crates/sms-api`'s `rotate_webhook_secret`
procedure (#41) is the intended caller of [`generate_secret`].
`examples/node/webhook-receiver` is the reference *receiver* — an
independent, from-scratch Node/TypeScript implementation of the exact
scheme below, kept deliberately separate so this module's own tests
(see `tests/cross_language_fixtures.rs`) can prove the two agree
byte-for-byte rather than merely being self-consistent with themselves.

# Headers

§4.4 of `docs/architecture.md` specifies four headers; the `hooks` role
must send exactly these, named exactly this way — [`HEADER_EVENT`],
[`HEADER_EVENT_ID`], [`HEADER_TIMESTAMP`], [`HEADER_SIGNATURE`] are the
constants for them. Use the constants, not a retyped literal, so a typo
can't silently diverge sender and receiver:

```text
X-Sms-Event:      message.delivered
X-Sms-Event-Id:   <WebhookAttempt.sourceEventId, lowercase-hyphenated UUID>
X-Sms-Timestamp:  <unix seconds, decimal, no sign, no leading zero>
X-Sms-Signature:  v1=<hex>[,v1=<hex>]   (current secret first, prevSecret second — oldest last)
```

# Canonical string — the exact bytes that get HMAC'd

```text
v1\n{timestamp}\n{eventId}\n{sha256_hex(body)}
```

- `{timestamp}` — the exact ASCII decimal string sent as
  `X-Sms-Timestamp` (base 10, no sign, no leading zeros — e.g.
  `1753699200`).
- `{eventId}` — the exact string sent as `X-Sms-Event-Id`, byte for
  byte.
- `sha256_hex(body)` — lowercase hex SHA-256 of the *raw* request body
  bytes, exactly as they go on the wire. Never a re-serialised or
  whitespace-normalised copy — that would silently diverge from
  whatever a receiver hashes off its own raw socket read.
- Fields are joined by a single `\n` (0x0A, LF only — no CR); there is
  no trailing newline after the fourth field. See [`canonical_string`],
  which is exposed publicly precisely so nobody has to reverse-engineer
  this prose back into bytes.

`HMAC-SHA256(key = secret, message = canonical string)`, lowercase hex,
is the value that follows `v1=` in `X-Sms-Signature`. See [`sign_v1`].

# Rotation

`X-Sms-Signature` may carry two `v1=` values — current secret first,
`WebhookEndpoint.prevSecret` second, "oldest last" per §4.4. A receiver
should accept if *any* presented value verifies against *any* secret it
holds; [`verify`] implements exactly that (every presented value
against every candidate secret, first match wins — order of
`candidate_secrets` doesn't matter to this function even though
[`sign_header`]'s caller-facing convention is "current first"). §4.4,
in its own words: "a job clears `prevSecret` after 24 hours" — that job
is out of this crate's scope (see `rotate_webhook_secret`'s own doc
comment in `backends/crates/sms-api/src/procedures.rs` for the current, narrower
scope cut this PR ships). Once it runs, `prevSecret` naturally stops
being a candidate because it is no longer stored anywhere — not because
of anything in this module.

# Public API (the contract `hooks` and `rotate_webhook_secret` code against)

```text
sign_v1(secret: &str, timestamp: i64, event_id: &str, body: &[u8]) -> String
sign_header(secrets: &[&str], timestamp: i64, event_id: &str, body: &[u8]) -> String
verify(candidate_secrets: &[&str], timestamp: i64, event_id: &str, body: &[u8], signature_header: &str) -> bool
canonical_string(timestamp: i64, event_id: &str, body: &[u8]) -> String
is_timestamp_fresh(timestamp: i64, now: i64, tolerance_secs: i64) -> bool
generate_secret() -> String
```

# Constant-time comparison

[`verify`] never compares a hex string or raw bytes with `==`. Every
candidate signature is checked via [`hmac::Mac::verify_slice`], which
performs a `subtle::ConstantTimeEq` comparison internally — the
standard defence against a timing side channel that would otherwise let
an attacker recover a valid signature one byte at a time by measuring
how long a comparison takes to fail.
