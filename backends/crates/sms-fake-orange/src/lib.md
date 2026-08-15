A fault-injecting fake of Orange Cameroon's SMS HTTP API.

Built to fuzz `vsms`'s message state machine for invariant violations —
the automatable complement to `docs/runbooks/36-handset-gate.adoc`, not a
replacement for it. **This crate cannot close #36**: it cannot tell you
Orange's real DLR payload shape, whether `receiptRequest` is genuinely
honoured, or whether a handset ever buzzes. What it buys is a permanent
regression net over the failure modes that *can* be modelled from public
`OneAPI`-family documentation and this repo's own hard-won findings about
how `OrangeCmProvider` classifies transport failures (§6.1/§6.2).

# Why a participant, not a response stub

"The SMS never arrived" is not a response — it's the *absence* of a
later callback. A fake that only answers the submit HTTP call can never
model that, or a DLR that arrives twice, out of order, for an unknown
reference, or racing the submit response it's nominally about. So this
crate owns three things, not one:

1. **Inbound stubbing** ([`FakeOrange::start`]) — the token endpoint and
   the submit endpoint, answered per [`fault::FaultPolicy`].
2. **A DLR scheduler** — a background `tokio` task per scheduled
   [`fault::DlrStep`], independent of the submit HTTP response, that
   POSTs a real `deliveryInfoNotification` body to whatever URL the
   caller wired up as the gateway's `POST /dlr/{providerKey}` route.
3. **A request ledger** ([`ledger::Ledger`]) — every submit call
   received, queryable by test code, so a test can prove "Orange
   received this reference exactly once" from the provider's own side
   rather than inferring it from this system's database.

# Two test policies, not a spectrum — plus one for a long-lived process

[`fault::FaultPolicy::Scripted`] is an exact, ordered sequence — what a
deterministic CI-gate test scripts to assert one specific outcome.
[`fault::FaultPolicy::Seeded`] is a seeded PRNG that draws a weighted mix
of realistic outcomes — reproducible by construction, since the same
seed replayed against the same call sequence always draws the same
decisions. Never unseeded randomness anywhere in this crate.

Neither fits a process that outlives any one test: `Scripted` exhausts
and falls back to a bare accept-with-no-DLR, and `Seeded` is tuned for a
fuzz sweep's tail coverage, not a demo's happy path. See
[`fault::FaultPolicy::Always`] for the third policy that exists
specifically for `backends/apps/sms-fake-orange`.
