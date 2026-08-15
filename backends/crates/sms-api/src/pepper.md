The server-held secret key behind `Message.msisdnHash`/`Message.bodyHash`
(#134).

`docs/architecture.md` has always *described* these columns as HMAC-SHA256
under a pepper. Until #134 the implementation was plain, unkeyed
`SHA-256` — reversible in seconds over Cameroon's ~10^7-candidate mobile
numbering space, which means a "purge" that clears `msisdn` but keeps
`msisdnHash` had not de-identified anything. This module is the fix: a
[`HashPepper`] newtype carrying real secret material (never the
database — `@sensitive`/`@pii` redact audit snapshots only, per
`AGENTS.md` §2.0, so a schema field could never have been a
confidentiality control here), and [`hmac_sha256_hex`], the one place
that turns a pepper + plaintext into the stored form.

# Stored form

`"{HASH_SCHEME}:{hex}"` — e.g. `hmac-sha256-v1:9f86d0...`. The old
`sha256:` prefix was, by its own doc comment, written specifically so a
future keyed scheme would be distinguishable per row; this is that
migration. The `-v1` suffix is deliberate, not decoration: a future
pepper *rotation* needs its own scheme tag (`hmac-sha256-v2:`, keyed
under a new pepper) so old and new rows are distinguishable by the
stored value alone, the same reasoning the original prefix existed for.

# No dual-read / rehash path exists, and none is being added

Per `AGENTS.md`, there is no live database anywhere in this deployment
yet. That makes this a clean cutover: every `sha256:`-prefixed value in
this tree is test fixture data, not production data, so there is
nothing to migrate and no dual-read path is worth building. If this
lands after real traffic exists, that assumption no longer holds — see
the rotation consequence below, which is the same problem in miniature.

# Rotation consequence (documented, not solved here)

Rotating the pepper — deploying a new [`HashPepper`] value — changes
every hash this process computes from that moment on, but does **not**
retroactively rehash a single already-stored row. The consequence is
asymmetric and worth spelling out precisely:

- A `Message`/`OptOut` row that still holds plaintext `msisdn` *can* be
  rehashed under the new pepper by a batch job that reads the plaintext
  and rewrites `msisdnHash` — no design for that job exists yet.
- A row whose `msisdn` has already been purged (the entire point of
  `msisdnHash` existing) cannot: there is no plaintext left to rehash
  from, so that row's hash is permanently stuck under the old pepper.
- Until any such rehashing happens, `OptOut` matching and dedupe against
  old rows silently stop working the instant the pepper rotates — a
  `msisdnHash` computed under the new pepper will never equal one stored
  under the old one, and nothing here detects the mismatch; it just
  looks like "not opted out" or "not a duplicate" to the day-one code
  that only ever compares hashes computed under whatever pepper is
  currently configured.

This is the same shape of tradeoff `ProviderError::Indeterminate`
documents elsewhere in this codebase: a real, accepted operational
consequence, written down rather than hidden behind a migration nobody
has designed yet.
