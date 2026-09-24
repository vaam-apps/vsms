# Showing a value

Every one of these exists because the same value was being formatted two or
three different ways on two or three different screens. Consistency is the
deliverable: an operator comparing a row against a ledger should not have to
work out whether the two amounts are written the same way.

## Identifiers and literals

### `IdDisplay`

An opaque machine identifier — a CUID, a ULID, a provider's reference.

```tsx
<IdDisplay value="cs_a1b2c3d4e5f6g7h8i9j0k1l2" />                 // table: first 7 chars
<IdDisplay value="cs_a1b2c3d4e5f6g7h8i9j0k1l2" variant="full" />  // detail: whole value
<IdDisplay value="pi_3QaBcDeFgHiJkLmN" truncateTo={12} />
```

| Prop | Type | Notes |
|---|---|---|
| `value` | `string` | The full value. Always what gets copied. |
| `variant` | `"table" \| "full"` | `table` (default) truncates and reveals the copy button on hover; `full` shows everything, `select-all`, copy always visible. |
| `truncateTo` | `number` | Leading characters in `table`. Default `7` — enough to tell two 23-character CUIDs apart in a column without dominating it. An id family with a shared prefix (`ord_`, `pi_`) needs more. |

Two rules it enforces so no call site has to:

- **Never truncate in the middle.** `abc…xyz` makes two ids that differ only
  in the middle look identical, which is exactly the comparison a human
  scanning a column is making.
- **Never decorate.** A displayed `msg_abc…` gets pasted into a filter that
  wanted the bare value and returns nothing, or a 400. If a prefix is part
  of the id, it belongs in `value`.

Copy always copies the *full* value in both variants. The truncated form is
for the eye only.

### `Code`

Inline monospace for a literal term that must be read in full — a config
key, a role key, a job kind, a field name.

```tsx
<p className="text-body text-muted-foreground">
  The route matched on <Code>prefix:+2376</Code> and chose <Code>orange_cm</Code>.
</p>
```

**Not `IdDisplay`**, and the distinction is load-bearing: `IdDisplay`
truncates, which is right for an opaque id you match by eye and wrong for
anything a person is meant to *read*. Renders a `<span>`, not a `<code>` —
several call sites sit inside a `<dd>` or a sentence where `<code>` would
add semantics the content does not have. Props: `children`, `className`.

### `CopyButton`

The copy affordance itself, if you need one somewhere the components above
do not put it.

```tsx
<CopyButton value="+237677123456" label="Copy payer number" />
<span className="group inline-flex items-center gap-1.5">
  <Code>whsec_live</Code>
  <CopyButton value={secret} revealOnGroupHover />
</span>
```

| Prop | Type | Notes |
|---|---|---|
| `value` | `string` | **The exact text put on the clipboard.** Always the full, machine-readable value — never what is displayed. A truncated id or a prettily-spaced phone number pasted into a query returns nothing. |
| `label` | `string` | Accessible name. Defaults to `Copy ${value}`, right for a short id and wrong for a paragraph. |
| `revealOnGroupHover` | `boolean` | Hide until the containing `.group` is hovered or this button is focused — for dense tables. Keyboard users always reach it: focus reveals it. |
| `size` | `12 \| 14 \| 16` | Icon size. Default `12`. |

A checkmark replaces the icon for 1500ms. A refused clipboard write — an
insecure origin, a permissions policy — leaves the icon unchanged and logs
the reason, rather than claiming a copy that did not happen.

### `PhoneDisplay`

A phone number, never truncated. Unlike an opaque id, no prefix of a phone
number identifies it, so a half-shown one is worse than useless in an
investigation.

```tsx
<PhoneDisplay
  value="+237677123456"
  format={(e164) => e164.replace(/^(\+237)(\d)(\d\d)(\d\d)(\d\d)(\d\d)$/, "$1 $2 $3 $4 $5 $6")}
  tag="MTN"
  tagTitle="Carrier inferred from the number prefix. Portability means this is wrong for some subscribers."
/>
```

| Prop | Type | Notes |
|---|---|---|
| `value` | `string` | E.164. The stored form, and what gets copied. |
| `format` | `(e164: string) => string \| null` | Regrouping for reading. Omit it, or return `null`, and the raw E.164 renders. |
| `tag` | `string` | A short mono tag after the number — carrier, line type, region. Renders an em dash when absent, so a column stays aligned. |
| `tagTitle` | `string` | Hover text for the tag. Say where it came from and how much to trust it. |

**There is no built-in grouping and no country table on purpose.** Grouping
is a per-country convention and a mis-grouped number reads as a *different*
number, so an application that knows its market passes a formatter and
everyone else gets the raw E.164, which is always correct if less pretty.
Copy always copies the raw E.164, not the grouped form on screen.

### `MaskedValue` (and `maskSecret`)

A secret, masked by default with an explicit reveal.

```tsx
<MaskedValue value="whsec_9f2a4c7e1b8d3a5f" prefix={6} reveal={4} label="signing secret" />
<MaskedValue value="JBSWY3DPEHPK3PXP" reveal={0} label="TOTP secret" />
<MaskedValue value="+237677123456" reveal={3} revealable={false} label="payer number" />
```

| Prop | Type | Default | Notes |
|---|---|---|---|
| `value` | `string` | — | The full value. In the DOM only while revealed. |
| `reveal` | `number` | `4` | Trailing characters left visible. `0` masks everything. |
| `prefix` | `number` | `0` | Leading characters left visible — for a prefixed credential (`whsec_`, `sk_live_`) where the prefix says what kind of thing it is and is not itself secret. |
| `revealable` | `boolean` | `true` | `false` gives a permanently masked value with no toggle. |
| `copyable` | `boolean` | `true` | Copies the full value whether or not it is revealed — the point of masking is the screen, not the clipboard. |
| `label` | `string` | `"value"` | Names the value in the toggle's and copy button's accessible names. |

**What is revealed:** the `prefix` leading characters, the `reveal` trailing
characters, and a **fixed-length run of eight dots** in between — the same
run no matter how many characters are actually hidden. That is the whole
point of the constant: the previous rule clamped the dot count to `[6, 12]`,
which is the identity function on exactly that range, so any secret with 6
to 12 hidden characters — the common case for an API key — published its own
exact length to anyone looking over a shoulder. When `reveal` covers the
whole value there are no dots at all.

**What this is not:** a security boundary. Whoever renders it already
received the value over the wire, and anyone with the session and dev tools
can read it. What masking buys is narrow and real — the value does not end
up in a screenshot, a recording, or the eyeline of the person behind the
operator. A value that must never reach the client is an API decision; no
component substitutes for it.

`maskSecret(value, prefix, reveal)` is the rule itself. It is deliberately
**not** exported from the package — it lives in `src/lib/mask-secret.ts` so
it can be unit-tested without `maskSecret`, one of the most generic names
available, landing on the published surface. Use the component.

## Numbers and time

### `Money`, `formatMoney`, `currencyExponent`

```tsx
<Money amount={500_000} currency="XAF" />                    // XAF 500,000
<Money amount={1250} currency="USD" />                       // USD 12.50
<Money amount="9007199254740993" currency="USD" />           // exact past 2^53
<Money amount={-32_500} currency="XAF" tone="signed" />
<Money amount={1250} currency="USD" display="symbol" />      // $12.50
```

| Prop | Type | Notes |
|---|---|---|
| `amount` | `MinorUnits` = `number \| bigint \| string` | An **integer count of the currency's smallest unit**. |
| `currency` | `string` | ISO 4217 code. |
| `tone` | `"none" \| "signed"` | `signed` tints credits green and debits red. Off by default: in a ledger where most rows point one way, colouring every row is noise, and colour alone is not an accessible way to carry sign. The minus is always rendered regardless. |
| `locale` | `string` | Defaults to the runtime's. Pass one wherever output must be stable — a test, a server-rendered page that must match hydration, a figure compared against a statement. |
| `display` | `"symbol" \| "code" \| "none"` | Default `code`. |
| `signDisplay` | `Intl.NumberFormatOptions["signDisplay"]` | e.g. `"always"` for a ledger delta. |

`code` is the default because symbols are ambiguous across locales — `$`
alone names at least a dozen currencies — and an operator reconciling
against a ledger needs the ISO code.

`tabular-nums` is the reason this is a component rather than a call to
`formatMoney` in a span: without fixed-width digits a column of amounts does
not line up at the decimal point, and scanning it for the outlier stops
working. It renders `<data value="500000 XAF">`, so a scraper or a paste
gets the exact integer rather than a locale-formatted string to parse back.

**Minor units in, never a decimal number.** `formatMoney(1250, "USD")` is
twelve dollars fifty; `formatMoney(12.5, "USD")` throws. So does a `number`
past `Number.MAX_SAFE_INTEGER`, because past 2^53 this function cannot tell
an exact value from one the caller's arithmetic already rounded — pass a
`bigint` or a digit string, which are unaffected at any size. Scaling is
string surgery, never division: the naive `9007199254740993 / 100` evaluates
to `…409.92` and loses the last cent.

**The exponent comes from Intl, not from a table.** `currencyExponent(code,
locale?)` returns the decimal places, `2` for a currency Intl has never
heard of. A zero-exponent currency — XAF, XOF, JPY, CLP, VND — is not shifted
at all, so `500_000` minor units renders as `500,000` and not `5,000.00`.
The three-decimal currencies are the tell that a hand-written table is in
use: KWD, BHD and TND are the ones every such table forgets.

One thing that catches test authors: Intl separates the code from the
number with a no-break space (U+00A0), deliberately, so the two cannot split
across a line break. `formatMoney` passes it through unchanged, so the
output does not compare equal to the visually identical string you would
type. Normalise in the assertion, not in the formatter.

### `TimestampDisplay`, `formatAbsolute`, `formatElapsed`

```tsx
<TimestampDisplay value="2026-09-11T14:03:07Z" />
<TimestampDisplay value={payout.createdAt} timezone="Africa/Douala" />
```

| Prop | Type | Notes |
|---|---|---|
| `value` | `string` | ISO 8601. |
| `timezone` | `string` | Any IANA zone name. Default `"UTC"`. |

Relative under 24 hours (`just now`, `2m`, `47m`, `6h` — under a minute is
`just now`, never `0m`), absolute otherwise. A relative stamp keeps the
absolute one in its `title` attribute, so hovering never leaves an operator
guessing. It renders the absolute form on the server and on first client
render so the markup matches, then upgrades to relative after mount. A single
shared 30-second interval drives every mounted instance, so a table of 200
rows does not run 200 timers.

The absolute form is `2026-09-11 14:03:07 Z` for UTC and
`2026-09-11 15:03:07 +01` for Africa/Douala — sortable, unambiguous, and
**with a space before the offset**, so a screenshot pasted into a ticket is
still interpretable. Offsets are read out of the zone and the instant, not
assumed from the zone name, so they stay right across DST
(`2026-01-15 09:03:07 -05` versus `2026-09-11 10:03:07 -04` for the same New
York clock) and for half-hour zones (`+05:30`, `+05:45`, `-09:30`). A zero
offset prints a bare Z rather than GMT.

The formatter behind it, `formatAbsolute(iso, timezone)`, and its sibling
`formatElapsed(ms)` — the gap renderer `StateTimeline` puts under each node
— live in `src/lib/format-instant.ts` and are **not exported from the
package**, deliberately: they were briefly public purely so a test could
reach them, and `formatAbsolute` and `formatElapsed` are far too generic to
sit on a published surface beside `formatMoney`. Their shapes, so you can
read a timeline: `+412ms`, `+1.000s`, `+1m 30s`, `+1h 30m`. A negative gap
renders with its real sign (`-5.000s`) rather than being clamped to zero,
because clamping would hide an ordering bug in the caller's data, and a
malformed one renders an em dash.

## Lists and panels

### `DetailList` + `DetailRow`

One `dt`/`dd` pair per row, in a `<dl>` that owns the spacing.

```tsx
<DetailList>
  <DetailRow label="Payout">
    <IdDisplay value="po_7h3k9m2q4x8b1v6n0z5" />
  </DetailRow>
  <DetailRow label="Amount">
    <Money amount={318_420_00} currency="XAF" />
  </DetailRow>
  <DetailRow variant="stacked" label="Provider reference">
    a-very-long-provider-supplied-reference-that-would-rather-not-wrap
  </DetailRow>
  <DetailRow label="Settled">
    <TimestampDisplay value="2026-09-11T14:03:07Z" />
  </DetailRow>
</DetailList>
```

`variant` is `"inline"` (default — label left, value right, one line),
`"stacked"` (label above, for a value too long to sit beside its label) or
`"divided"` (label above, hairline rule beneath, last row unruled). Both
components take it; `DetailList` uses it only to decide spacing, since
`divided` rows carry their own rhythm through padding and the rule, so the
list adds no gap for them.

**Mixing variants in one list is safe**, and that is a deliberate property.
All three now share one type pairing — label `text-caption
text-subtle-foreground`, value `text-body text-foreground` — so the variant
a row was picked for is invisible in its type. Before that, `stacked`
rendered its label *larger* than the value it labelled and `inline` styled
neither side, so one drawer showed values at two or three different sizes
depending on which layout each row happened to need. Pick the variant for
the shape of the value, not for how the text should look.

### `StatTile`

One headline number with its label and context.

```tsx
<StatTile label="Delivered" value="12,481" caption="98.2% of 12,710 terminal" />
<StatTile label="Unresolved" value="37" tone="uncertain" caption="Outcome never learned" />
<StatTile
  label="Spend"
  value={<Money amount={318_420_00} currency="XAF" display="none" />}
  caption="XAF, across all providers"
  action={<StateChip tone="progress">live</StateChip>}
/>
```

| Prop | Type | Notes |
|---|---|---|
| `label` | `React.ReactNode` | Truncates to one line. |
| `value` | `React.ReactNode` | The figure. **This component does no formatting** — a tile cannot know whether it is showing a count, a currency or a rate. Pass a `Money` or a formatted string. Never clamped: a truncated figure is a wrong figure, so a long one wraps inside the tile instead. |
| `caption` | `React.ReactNode` | One line under the value: the comparison, the window, the caveat. Clamped to two lines — tiles stretch to the tallest in the row. |
| `tone` | `StatusHue` | Tints the value. Leave unset unless the number's own colour carries meaning; a wall of coloured tiles makes the one that matters harder to find. |
| `emphasized` | `boolean` | Singles this tile's value out among peers by weight (`font-medium`), not colour — colour is reserved for status. Set it on at most one tile in a row; a row where every tile is emphasised has no emphasis. |
| `action` | `React.ReactNode` | Top-right slot — a sparkline, a `StateChip`, a refresh button. |

The `caption` slot exists because a bare number with no denominator is the
most common way a dashboard misleads. It is deliberately upright sans, not
italic: these strings are emitted facts of the same register as the figure
above them, not someone's commentary.

Borders, not shadows, and `<dl>`/`<dt>`/`<dd>` rather than divs: tiles
appear in rows of three to six, six shadowed boxes is noise where six
outlined ones read as one instrument, and the semantics make a row of them
navigable to a screen reader instead of a wall of text.

### `InstrumentPanel`

The **instrument register**: an aurora mesh ground, no border, for data you
*scan* rather than read.

```tsx
<InstrumentPanel title="Settlement" caption="Last 24 hours, across all providers">
  <div className="grid gap-3 sm:grid-cols-3">
    <StatTile label="Paid" value="12,481" caption="98.2% of 12,710 terminal" />
    <StatTile label="Unresolved" value="37" tone="uncertain" caption="Outcome never learned" />
    <StatTile
      label="Spend"
      value={<Money amount={318_420_00} currency="XAF" display="none" />}
      caption="XAF, across all providers"
    />
  </div>
</InstrumentPanel>
```

`title` and `caption` are both `React.ReactNode` and optional; everything else
spreads onto the `<div>` (`title` is deliberately omitted from the DOM
attributes it accepts, since this slot is a node and the attribute is a
string). `Card` with `glow` is the same register at card scale.

Three things to know:

- **Use it sparingly.** A screen where every panel glows has no glow. The
  library's default surface is a hairline on a surface step, and should stay
  most of your screen.
- **The mesh carries no meaning.** It is drawn from the `--aurora-*` ramp,
  bound to the system's own hues so it cannot drift, but it is chrome: no
  status is inferable from it, there is no per-state variant, and a caller
  cannot tint it. That is the only reason a coloured surface is safe in a
  system whose premise is that colour means something.
- **`text-subtle-foreground` is banned on this surface.** A gradient ground
  breaks the assumption every contrast check here rests on — that a
  foreground sits on *one* known surface. Measured over `--surface-1` at the
  shipped 14% cap, worst case across the four blobs, subtle falls to
  **4.41:1 in dark and 4.45:1 in light**, below the 4.5:1 bar, while muted
  holds at 5.29:1 and above. The component already steps its own caption up
  to muted; do not push it back down, and do not reach for subtle on
  anything else you put here.
