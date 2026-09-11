# Multi-country: Cameroon first, extensible to anywhere

**Status:** decided and staged. Supersedes the single-country half of `docs/architecture.md`'s own
scope line ("a self-hosted A2P SMS gateway for Cameroon (MTN + Orange)").

## Why this exists

vsms was scoped to Cameroon before any code was written, and every decision since compounded on
that. The scope has changed: prospective customers sit in Europe, the USA, Canada, South Asia and
elsewhere in Africa. This document records what that costs, what it does not touch, and which things
are deliberately left unsolved rather than half-solved.

**The principle, and it constrains every decision below: Cameroon stays first.** A Cameroon
deployment must not get slower, or more configurable-by-necessity. Nothing here makes `+237` a
special case that has to be configured; it stays the default. Every other country becomes reachable
through data and configuration rather than a code change, except where that country's own law
genuinely demands code.

## What was measured before deciding

None of the following is taken from a crate description. Each was run against
`phonenumber 0.3.10+9.0.33` (Apache-2.0, carrying Google libphonenumber 9.0.33 metadata) and
`iso_currency 0.7.1` in the `vsms-ci` VM.

**Cameroon behaviour is reproduced exactly, including the two edge cases this repository was most
careful about.** `sms-msisdn`'s hand-rolled plan gets two things right that a naive implementation
misses: the `63x`/`643`–`649` blocks are syntactically well-formed but unallocated, and the `88x`
toll-free range is 8 digits where every other number is 9. libphonenumber agrees on both, unprompted:

| input | today's `sms-msisdn` | libphonenumber |
|---|---|---|
| `+237677123456` | `Mobile` | `Mobile`, valid |
| `+237222123456` | `FixedLine` | `FixedLine`, valid |
| `+237631234567` | `Unallocated` | `Unknown`, **invalid** |
| `+23788123456` | `TollFree` | `TollFree`, valid |
| `677123456`, `00237677123456` | normalised | normalised identically |

That is a validation of the existing crate, not a reason to distrust it. The swap widens it rather
than correcting it.

**North America cannot be classified the way Cameroon can.** `+14155552671` and `+15145551234` both
return `FixedLineOrMobile`, not `Mobile`, because a North American Numbering Plan number does not
carry that distinction at all. This is the most consequential finding here: see decision 2.

**The currency trap is real and solvable.** `iso_currency` reports a minor-unit exponent of 0 for XAF
and JPY, and 2 for EUR, USD, INR and NGN. A system that assumed integer XAF everywhere mishandles
cents the first time it prices a European route.

**It fits the images.** No `-sys` crate, no C toolchain, 57 transitive crates. The metadata is
`include_bytes!` from `OUT_DIR` at compile time, with no runtime filesystem or network access, so it
works under `gcr.io/distroless/static`. A static musl build under `rust:1.98-alpine3.22`, the
toolchain `backends/apps/*/Dockerfile` actually uses, finished in 21 s and produced a 4.79 MB binary.
Against the current ~45 MB gateway image the metadata costs roughly 10%, paid once.

## Decision 1 — number parsing moves to libphonenumber metadata

`sms-msisdn` keeps its `Msisdn` newtype, its masking, its `LineType` vocabulary and its place as a
pure crate with no framework dependency. What it loses is the compiled-in `COUNTRY_CODE = "237"` and
the hand-written `classify_nine` block table, replaced by metadata covering every country.

**Rejected: hand-roll a second, third and fourth national plan.** There are around 240 of them, they
change, and getting Cameroon right took real care for a plan that is unusually simple. Doing it by
hand defers the cost onto whoever onboards each customer and guarantees drift, because nobody tracks
ITU amendments manually.

## Decision 2 — `parse_mobile` must accept `FixedLineOrMobile`, or every US and Canadian number is rejected

Today `Msisdn::parse_mobile` accepts only `LineType::Mobile`. Carried over unchanged, that rejects
**every North American number**, because the NANP genuinely does not encode the distinction. A long
tail of other countries behaves the same way.

So addressability, not mobility, becomes the test: a number is addressable when it is `Mobile` or
`FixedLineOrMobile`. Cameroon is unaffected, because CM metadata returns `Mobile` for mobile ranges
and `FixedLine` for `2xx`, so a Cameroonian fixed line is still refused exactly as today.

This is the kind of change that ships looking correct and fails on the first American customer, which
is why it is recorded as its own decision rather than folded into decision 1.

**A second, quieter rejection sits beside it.** `Message.msisdn` is bounded `@length(min: 12, max: 15)`,
a floor derived from `+237` plus nine digits. It excludes real E.164 countries outright: Denmark,
Norway and Iceland all produce 11-character numbers. A Danish customer's traffic is refused before it
reaches a provider, for a reason that has nothing to do with Denmark. The bound carries no
`@db_enforce`, so it is a schema edit with no migration, and it belongs with stage 1 rather than
later. `PURGED_MSISDN_PLACEHOLDER` is sized to satisfy that same bound and moves with it.

## Decision 3 — there is no global number-to-operator mapping, and the design stops implying one

The `phonenumber` crate ships no carrier mapper or geocoder; its `Carrier` type is a carrier-selection
prefix parsed out of the input, not a lookup. That is not an omission. Mobile number portability
means a prefix stops identifying a network the moment a subscriber ports, and in the USA, UK and
India porting is routine.

`docs/architecture.md` §3.4 already says prefix routing "must never be load-bearing", and
`OperatorPrefixTable` already ships with zero built-in rows. That caution was right, and now becomes
mandatory:

- **Routing keys on destination country first, operator second.** A route may still match an operator
  where that is known and meaningful, which in practice means Cameroon and other markets where prefix
  data has been seeded and validated.
- **`operator` is honestly unknown for most foreign traffic**, and every consumer must handle that.
  The enum has always had an `unknown` variant; it stops being an edge case.
- **Per-operator analytics degrade to per-country abroad.** `sms_route_delivery_divergence` compares
  routes that should behave identically; grouping by country and class is the portable form.

## Decision 4 — country becomes a first-class column

There is no notion of country, region or locale anywhere in the schema today. `App` has no such
field, so there is nothing to hang a second country off. That absence, not the operator enum, is the
real blocker.

- `App.defaultCountry` — ISO 3166-1 alpha-2, defaulting to `CM`. The region used to interpret a bare
  national number from that tenant, which is what keeps `677123456` working unchanged for a
  Cameroonian app while `4155552671` means something different for an American one.
- `Message.destinationCountry` — stamped at accept time from the parsed number. Routing, compliance
  and reporting all need it, and deriving it later from an already-purged `msisdn` is impossible.
- `Route.matchCountry` — the primary routing predicate, alongside the existing operator and class
  predicates.

**`OperatorPrefixRule.prefix` is globally `@unique`, and that is the hardest schema obstacle in the
inventory.** The prefix table is the entire operator-classification mechanism, and a national prefix
may exist exactly once across a deployment. Cameroon's MTN `67` and a French `67` cannot coexist,
regardless of how country-agnostic the lookup code is, and the lookup code genuinely is: it takes
untyped string pairs and does longest-prefix matching with no country concept. The model needs a
country discriminator and the unique index needs to become composite. Both ends of that mechanism are
bound while its middle is already portable, which is the clearest illustration in the codebase of
what this whole program is doing.

`SenderId.value` is globally `@unique` too, so two customers in two countries cannot both register
`INFO`. That one is listed under what is not solved, because it is entangled with sender-ID regimes.

## Decision 5 — money carries its currency

`costXaf`, `costPerSegmentXaf` and `estimatedCostXaf` encode the currency in the **field name**, so it
reaches the DDL, the generated REST surface, both SDKs and the admin console. A gateway that prices a
European route cannot express what it costs.

Each becomes an amount plus an ISO 4217 code, with the minor-unit exponent read from the currency
rather than assumed. Two prices may only be compared or summed when their currencies match. The
alternative, a single reporting currency, needs an FX rate source and a rate-as-of date, which is a
product decision nobody has made.

**This breaks the published wire contract**, so it lands as v0.4.0. That suits this repository's
standing preference for a hard cutover over a parallel path, and it is cheapest now: there is still
no live database anywhere, and the Node and Rust SDKs have no third-party consumers. The Rust SDK
vendors the schema byte-for-byte, so the enum and every `*Xaf` name are in its published typed
surface, not merely adjacent to it.

**It is also a smaller change than it looks, because the billing path does not exist.** `Message.costXaf`
has no writer anywhere in the repository. It defaults to zero, stays zero, and serialises into every
webhook as `"0"`, while `procedures.rs` claims it "is the number that bills" and `docs/integrating.md`
advertises `"costXaf": "22.00"`. Both are wrong and are corrected as part of this work. So currency is
close to greenfield rather than a migration, and money precision was never the problem: amounts are
`Decimal` over unbounded `NUMERIC` end to end, with no zero-decimal assumption anywhere.

**The real defect the audit found here is a comparison, not a precision loss.** Provider selection
orders by `costPerSegmentXaf` ascending to pick the cheapest active provider. That comparison is
currency-blind, so the moment a second currency exists it ranks XAF against EUR as though the numbers
were commensurate and routes traffic to whichever currency happens to have smaller numerals. Adding a
currency column without fixing that ordering would be worse than leaving it alone.

## Decision 6 — operator becomes data rather than a DDL enum

`OperatorCode` is a closed enum of four Cameroonian carriers plus `unknown`, enforced by a `CHECK`
constraint on five columns, and mirrored again as a hand-copied enum in the pure `sms-routing` crate
and four more times across the console and SDKs. Globally there are over a thousand networks. It becomes a nullable key
into a `MobileNetwork` table scoped by country; Cameroon keeps `mtn`, `orange`, `camtel` and
`nexttel` as its keys, so existing semantics carry over unchanged.

Doing this now is close to free and expensive later, precisely because no database holds data yet.

**Rejected: keep the enum and add variants per market.** That turns every new customer into a schema
migration and a released-SDK change, which is the opposite of extensible.

## Decision 7 — per-country compliance is a table, not a constant

`MARKETING_QUIET_HOURS_START_WAT` and `_END_WAT` are constants with a hardcoded UTC+1 offset. Quiet
hours are jurisdictional, and the rules differ in kind rather than degree: the USA sets them in the
recipient's local time, India layers a national do-not-disturb registry on top, and the EU largely
regulates consent instead of hours.

These become seeded reference data carrying, per country, a timezone, a quiet-hours window where one
applies, and the sender-ID regime. Code keeps a conservative fallback for a country with no row, so
an unseeded country fails closed rather than sending at 03:00 local time.

**The window is the easy half; the arithmetic is the trap.** Today the hour is computed by adding a
fixed `+1` to UTC, which is right for Cameroon because it observes WAT year-round with no DST. A
per-country table of fixed offsets inherits the bug rather than fixing it: it is wrong twice a year
for the EU, the USA and Canada, and wrong always for India, which sits at UTC+5:30. This needs a real
IANA timezone database, which is a new dependency rather than a constant swap. The same absence shows
up in the monthly quota window, which uses UTC and says so in its own comment, because nothing in the
schema carries a timezone.

## Explicitly not solved here

Naming these matters more than the decisions above, because each is a real obstacle for a specific
market and none is fixed by the staging below.

- **Sender ID regimes.** Alphanumeric sender IDs are ordinary in Europe and much of Africa,
  effectively unusable in the USA without 10DLC, toll-free or short-code registration, and require
  TRAI DLT pre-registration of both sender and template in India. `SenderId` and
  `SenderIdRegistration` are already per-(sender, provider); they gain a country dimension, but the
  registration workflows themselves are per-market projects. Three concrete obstacles, all verified:
  `SenderId.value` is bounded 3 to 11 characters by a real database `CHECK`, so a US 10DLC long code
  structurally cannot be stored; `SenderIdKind` is read by nothing, so the alphanumeric-versus-shortcode
  distinction is a label rather than a control; and the `supports*` capability columns on `Provider`
  are written by three seed paths and read by none, so nothing could refuse an alphanumeric sender for
  a country that bans it even if the value fit. India additionally pre-registers message *templates*,
  and no template model exists.
- **Providers with real coverage.** `sms-provider-orange-cm` is Cameroon-specific, and
  `sms-provider-mtn`'s wire shape is still an unverified placeholder. Serving Europe or North America
  means a real aggregator, which is the config-driven `AggregatorHttpProvider` that §6.2 specified
  and nobody built.
- **Data residency conflicts.** §10's 90-day minimisation is reasoned from Law No. 2024/017 and
  Law 2010/012. Another jurisdiction can mandate retention where Cameroon mandates erasure, and the
  hosting question stays open. Multi-country probably means multi-deployment rather than one database
  holding every market's traffic.
- **STOP keyword handling** (#76) is a compliance requirement in the USA and Canada, not a feature.
- **Console translation** (#231) is English-only today.
- **GSM-7 national language shift tables.** South Asian scripts already fall to UCS-2 correctly, so
  this is a cost optimisation for Turkish, Spanish and Portuguese rather than a correctness gap.

## Staging

Each stage is independently shippable and independently useful.

| stage | change | breaks |
|---|---|---|
| 1 | `sms-msisdn` parses every country, Cameroon stays the default region | nothing |
| 2 | country as a column: `App.defaultCountry`, `Message.destinationCountry`, `Route.matchCountry` | schema |
| 3 | money carries its currency | schema and published wire, v0.4.0 |
| 4 | operator becomes a `MobileNetwork` reference | schema |
| 5 | per-country policy table replaces the WAT constants | schema |

Stage 1 alone lets a customer's numbers be parsed and validated correctly, which is the gate
everything else sits behind.
