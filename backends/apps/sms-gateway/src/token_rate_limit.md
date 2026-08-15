#168: a defence-in-depth `/token` rate limiter keyed on the real
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
