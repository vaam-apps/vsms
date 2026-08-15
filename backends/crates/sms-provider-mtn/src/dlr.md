Parsing the aggregator's delivery notification callback.

**Not verified against a live aggregator sandbox** — see `lib.rs`'s
module doc for the full honesty ledger. The shape assumed here is a
single JSON object per callback (not batched — some providers batch,
per [`sms_provider::SmsProvider::parse_dlr`]'s own doc, but nothing in
`docs/architecture.md` suggests this route does, so the simpler shape
is the default until a real payload says otherwise):

```json
{
  "messageId": "mtn-res-42",
  "status": "DELIVERED",
  "errorCode": "...",
  "network": "mtn",
  "occurredAt": "2026-08-11T12:00:00Z"
}
```

`messageId` is the same value `submit()` (`lib.rs`) returns as
`SubmitAck::provider_ref` — unlike Orange, this assumed shape needs no
`callbackData`/`provider_ref_alt` workaround, because the invented
contract is that the aggregator echoes its own id back rather than a
caller-supplied correlation token. If a real aggregator's DLR turns out
to reference the submission a different way, this is precisely the kind
of correlation gap #95 already shows this codebase can ship silently —
revisit this module and `submit`'s `provider_ref_alt` together.
