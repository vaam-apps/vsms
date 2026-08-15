The CAS claim loop every claiming role shares. §7.3 of the design doc.

`SKIP LOCKED` is not expressible through the framework — verified by grep
across every crate and by compile error on `skip_locked()` (§7.3).
`.for_update()` exists but blocks rather than skips, which for a claim
loop means workers queuing behind each other instead of moving on. So
every claim here is optimistic CAS on `@version` instead: select
candidates, take a lease with `if_match(version)`, and read the outcome —
`PreconditionFailed` means another worker won a race that was always
going to have a loser, `Forbidden` means something worth knowing about
happened, and anything else is a real failure.

[`Claimable`] is what makes [`claim_batch`] one function rather than one
per model — "the job and webhook claims are the same function with
different types" (§7.3). This module implements it for [`Message`]
(`dispatch`'s claim, based on §7.3's own worked example, with two
corrections — see the doc comment on its `candidates` impl), for [`Job`]
(`jobs`'s claim, #35), and for [`WebhookAttempt`] (`hooks`'s claim, M3
#40 — see that `impl`'s own doc for how it differs from the other two:
endpoint health, not just row state, decides what's claimable).
