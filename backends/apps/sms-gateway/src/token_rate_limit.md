Issue 168 (#168): a defence-in-depth `/token` rate limiter keyed on the real
`client_id` — the composite dimension `docs/architecture.md` §4.2
requires (`client_id` **and** source IP) that `deploy/Caddyfile`'s
edge-level zones (#156) structurally cannot reach.

**Why this exists even though §4 originally called `/token` rate
limiting "an infrastructure concern, not a `sms-auth` one" (#22).**
That reasoning is not being silently reversed — it was correct for what
it decided, which was Argon2id `DoS` amplification (gone: `private_key_jwt`
has no password hashing at `/token` at all) and ordinary endpoint abuse
(the edge's job, and #156 built it). What #22 didn't anticipate,
because the edge didn't exist yet to reveal it, is narrower and
concrete: `deploy/Caddyfile`'s own `#168` comment documents, with
receipts, that Caddy **cannot** read `client_id` out of a
form-urlencoded POST body without either an inefficient body-buffering
trick that risks breaking `reverse_proxy`'s own forwarding, or a
single-contributor, zero-star third-party module — both rejected on
their own merits, not hand-waved away. The edge keeps doing flood
protection (`token_per_ip`, `token_global`); this module supplies
exactly the dimension the edge cannot reach, from the one place in this
system that already has the parsed body: after `axum::body::to_bytes`
has buffered it, immediately before the real OAuth handler runs.

**Mounted in `backends/apps/sms-gateway/src/op.rs`, not `authkestra-op` or
`sms-auth`** — `op.rs` already owns route assembly for everything under
`/token`/`/jwks.json`/`/.well-known/openid-configuration` (see that
module's own doc), and this is one more route-scoped concern alongside
the ones already there, not a change to the OAuth library or the
`ClientStore`/`OpStore` plumbing `sms-auth` owns.

**Buffers and reconstructs the body — never consumes it.** A `/token`
handler that receives an already-drained body breaks every real
exchange, not just an attacker's, which is the exact failure mode this
module's own tests assert against directly (see
`the_real_body_reaches_the_inner_handler_unmodified` below).

# The `authorization_code` grant is keyed on its code (VSA-4a)

The console redeems its codes as a **public** client, so its `client_id` is a value
anyone can type. Keyed on it, the grant would share one bucket among every operator:
`grant_type=authorization_code&client_id=sms-console&code=x`, repeated, empties it,
and every real code (sixty seconds of life, single use) expires behind a `429`.
That was the shape this module was first extended with, and a review found it.

So the key of an `authorization_code` request is **the SHA-256 of its code** and
nothing else (`limit_key`). Why that, of the two ways out (exempting the grant and
capping it in aggregate, or keying it per code):

- An aggregate cap is a bucket an attacker can empty, which is the lockout again
  with a larger number. Per code there is nothing shared to empty: a bad code
  spends its own bucket, a real code is a different key, and a 32-byte random code
  is not guessable in a retry loop, so the per-code budget exists to stop *one code*
  being hammered, not to slow the console.
- A request with no `code`, or two (the handler's form parser refuses a repeated
  field), shares one bucket **per shape**, and no real redemption ever lands in
  either: a real one has exactly one.
- The code is hashed so the credential never sits in the store as a key; it is a
  credential for sixty seconds.

What this does not bound is a caller who sends a stream of *distinct* bad codes,
each a fresh bucket and each a client lookup. That is the same exposure an unknown
`client_id` has always had here (each is a bucket of its own), it costs a read and
no hashing, and the edge's `auth_per_source` zone is what bounds a source that does
it. Every other grant is keyed as before.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
