The HTTP-status → [`sms_provider::ProviderError`] mapping that turned out,
on inspection while building this crate, to be genuinely identical between
`sms-provider-orange-cm` and `sms-provider-mtn` for every status neither
crate treats as a special case:

- `429` → `Transient`, always with the same `"rate limited: {body}"`
  message text in both crates (only the retry delay differed: Orange's 1s
  vs the MTN aggregator adapter's 5s — both §6.2/§6.4-derived commercial
  facts, not something this function should guess, so it's a parameter).
- A `5xx` → `Unavailable`, with the same `"{provider} returned {status}:
  {body}"` shape in both (only the provider noun differed: `"orange"` vs
  `"aggregator"`, both lowercase — preserved verbatim as each crate's own
  parameter, not normalised, since normalising it would be a silent text
  change this cleanup was explicitly told not to make).
- Everything else → `Rejected { code: "http_{status}", message: body }`,
  byte-for-byte identical in both crates, no parameters needed at all.

What stays where it was, and why — each adapter's own `classify_submit_error`
still owns:

- **MTN's `401`/`403` → `Permanent`.** Orange's own submit endpoint never
  had an equivalent branch: it authenticates via a separately-fetched
  bearer token (`token.rs`), so a `401`/`403` on the *submit* call itself
  is not a documented failure mode for that adapter and would fall through
  to the shared `Rejected` default exactly as it always has. MTN's
  aggregator sends its API key on every request, so an auth failure there
  is a live case with its own correct mapping (`Permanent`, not `Rejected`
  — the key is bad, but the message itself may be sendable through a
  different provider). Folding this into the shared function would force
  Orange to either grow an identical, currently-untested branch or accept
  behaviour it never had; leaving it local costs one `if` per adapter and
  changes nothing either adapter already does.
- **Whatever a real aggregator's `400` for "unapproved sender" turns out
  to look like**, per each crate's own already-documented honesty about
  not yet being able to distinguish that case from any other `400` — see
  `sms-provider-orange-cm`/`sms-provider-mtn`'s own module docs. Nothing
  to extract yet, because neither crate has anything more specific than
  the shared `Rejected` fallback for this today.

Structured as a total function (`ProviderError`, never `Option`) rather
than "return `None` for anything you have no opinion about, let the caller
fall through": every status this function was ever asked to classify by
either adapter, before this crate existed, mapped to exactly one of the
three cases above once each adapter's own special cases (MTN's 401/403)
were checked first. A caller with a genuine special case checks it *before*
delegating here, the same order both adapters already used.
