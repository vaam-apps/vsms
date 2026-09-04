One classifier every HTTP `SmsProvider` adapter in this workspace shares,
factored out once a second adapter (`sms-provider-mtn`, #61) proved the
first one (`sms-provider-orange-cm`, #33/#94/#119) wasn't a one-off shape.

AGENTS.md's own #61 section named this explicitly, before it was built:
*"`classify_transport_error`'s connect-vs-read logic is now byte-for-byte
the same reasoning in two crates... Worth doing the moment a third HTTP
adapter ... needs the same logic a third time."* Two crates duplicating it was a
deliberate, recorded decision, not an oversight — #61 wasn't the moment to
refactor the trait crate for one instance. This crate is that moment,
brought forward from "the next real adapter" to a standalone cleanup, since
the duplication was already fully proven correct in both places and the
extraction itself carries no behavioural risk (see [`transport`] and
[`submit_status`]'s own doc for exactly what did and did not change).

Two modules, two different layers of an HTTP submission's failure surface:

- [`transport::classify_transport_error`] — a failure from `.send()`
  itself, before any HTTP response existed at all. Connect-vs-read is the
  whole story: `reqwest::Error::is_connect()`, `is_timeout()`, `is_body()`
  answer it, and that answer follows from what `reqwest` guarantees, not
  from anything provider-specific — see #119's own reasoning in AGENTS.md
  for why getting this backwards risks a duplicate SMS.
- [`submit_status::classify_common_submit_status`] — the HTTP-status →
  `ProviderError` mapping that turned out, on inspection, to also be
  identical across both adapters for every status neither one treats as a
  special case (`429` → `Transient`, `5xx` → `Unavailable`, everything else
  → `Rejected`). Each adapter's own `classify_submit_error` still owns
  whatever *is* provider-specific — see that module's own doc for exactly
  which statuses that is and why.

## Why a separate crate, not a module inside `sms-provider`

`sms-provider`'s own module doc is explicit: *"Pure, like `sms-encoding` and
`sms-msisdn`: no `cratestack` dependency, no schema types"* — and the trait
it defines is deliberately described as `"HTTP or SMPP"`, not HTTP-only.
`reqwest` is an HTTP client; adding it to `sms-provider` itself would mean
every future SMPP-only adapter (and every consumer of the trait, including
`sms-worker`, which will eventually hold both kinds of adapter behind the
same `Arc<dyn SmsProvider>` registry) pulls in an HTTP client and its TLS
stack it has no use for. `reqwest::Error`-shaped classification is an
HTTP-transport concern, not a generic provider-abstraction one — SMPP's own
connect/write distinction (named as the other named case AGENTS.md's #61
section gives for "a third HTTP adapter... or SMPP's own connect/write
distinction needs the same logic a third time") will need a *different*
classifier over a *different* error type entirely, so it earns its own
crate (`sms-provider-smpp`, when it exists) rather than a second function
bolted onto this one.

What *does* belong in `sms-provider` itself, and stays there:
`ProviderError::{Unavailable,Indeterminate}::source` — a boxed
`dyn std::error::Error + Send + Sync + 'static`, so this crate (and a
future SMPP-transport crate) can attach a real underlying cause without
`sms-provider` needing to know or care what crate that cause came from.
`std::error::Error` is `core`/`std` only; naming it costs `sms-provider`
nothing in dependencies, so the "no cratestack, no schema types" purity
claim stays true even though the error surface got strictly better.
