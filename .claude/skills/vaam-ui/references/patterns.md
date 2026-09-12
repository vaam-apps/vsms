# Screen patterns

Composed building blocks — still domain-free, but opinionated about layout
in a way the primitives are not. Each one was hand-rolled in two or more
route groups before it was extracted, and the doc for each says which
decision it froze.

## Notices

### `InlineBanner`

A standing notice or an error *around* content that is showing.

```tsx
<InlineBanner>Scoped to the last 30 days. Older payouts are in the archive.</InlineBanner>

<InlineBanner variant="danger">
  The provider rejected this payout: <Code>INSUFFICIENT_FLOAT</Code>. Nothing was moved.
</InlineBanner>

<InlineBanner variant="uncertain">
  The live feed has not produced an event for 4 minutes. Figures below may be stale.
</InlineBanner>
```

`variant` is one of:

| Variant | For |
|---|---|
| `neutral` (default) | A standing notice — a scope, a caveat. |
| `danger` | An error. |
| `warning` | Recoverable, needs attention — a stale write. |
| `success` | A positive confirmation — a verified audit chain. |
| `uncertain` | Degraded or unknown, neither failure nor success — a stalled feed, a volume spike. |
| `plain` | A caption-only note, no border or fill, for something that does not need the weight of a box. |

One thing worth knowing, because it looks like a bug and is not:
`success` draws the same achromatic chrome as `neutral` — `border-edge` and
`bg-surface-2` — keeping only its own green text. `--state-success-bg` and
`--state-success-border` are declared `transparent` on purpose (success is
one of the two quiet status hues), so painting them rendered a bare line of
green text floating between five bordered siblings. Do not "fix" that by
reaching for the tokens directly.

Not to be confused with `InlineEmptyState`: that one is for "there is
nothing to show here" inside a list.

### `StaleWriteBanner`

The banner for a `412 Precondition Failed` save — "someone else changed this
row since it loaded".

```tsx
{isStaleWrite(error) && <StaleWriteBanner onReload={() => refetch()} />}

<StaleWriteBanner
  message="This payout was released by another operator while you were editing it."
  onReload={reload}
/>
```

`onReload: () => void` is required; `message` defaults to *"Someone else
changed this row since it loaded. Reload to see their edit."*

It renders through `InlineBanner` at `warning`, **not `danger`**, and that is
the decision it encodes: nothing was lost and nothing is broken. The write
was refused precisely so the other operator's edit survives — the mechanism
working, not failing — and reloading resolves it. Its Reload button is
explicitly `type="button"`, because both original call sites rendered it
inside a `<form>`, where the default type is `submit` and "Reload" would
have submitted the very edit the banner is warning has gone stale.

### `InlineEmptyState`

"There is nothing here", as a status line rather than a centred placard.

```tsx
<TableRow>
  <TableCell colSpan={6}>
    <InlineEmptyState message="No payouts match these filters." />
  </TableCell>
</TableRow>

<InlineEmptyState
  message="No routes configured."
  action={{ label: "Seed a catch-all", onClick: seedCatchAll }}
/>
```

| Prop | Type | Notes |
|---|---|---|
| `message` | `React.ReactNode` | One line. |
| `action` | `{ label: string; onClick: () => void }` | Optional, rendered as an underlined text button beside the message. |
| `variant` | `"inline" \| "standalone"` | `inline` (default) sits in a table body or the panel where the missing list would be. `standalone` centres a single line plus one action — the one exception, for a screen with nothing else to do. Still no illustration, still no card. |

The message stays upright sans, not italic, deliberately: the component is
reporting its own state, the same register as an empty cell or a skeleton,
rather than prose a person wrote for a reader. Italic in this system marks
someone addressing the operator, and it only means that because things like
this are excluded.

## Waiting and live data

### `RouteSkeleton`

The fallback for the `<Suspense>` boundary a route needs — a title bar, an
optional filter row, and some table-row-shaped blocks.

```tsx
<Suspense fallback={<RouteSkeleton rows={8} />}>
  <PayoutsTable />
</Suspense>

<RouteSkeleton rows={4} withFilterBar={false} />
```

`rows` defaults to `6`, `withFilterBar` to `true`.

It exists because those boundaries were passing `fallback={null}`, and a
`<Suspense>` reveal is `requestAnimationFrame`-gated by React's own
completion script. A backgrounded, non-composited tab never runs that
callback — proven live against a real build — so `null` rendered as a
permanently empty `<main>` with no signal that anything was ever happening.
This does not fix that (nothing in application code can make a browser paint
a hidden tab); it makes the wait *visible* instead of indistinguishable
from broken.

It is also the one place in the library that **announces** a wait. `Skeleton`
is `aria-hidden` — a dozen empty boxes read aloud is worse than silence — so
a `<main>` that is entirely skeletons would otherwise be silent to a screen
reader for as long as it is suspended. This one wraps itself in a
`role="status"` region that says "Loading" once and goes quiet when the real
tree arrives. Do not add your own pulse or your own live region on top.

### `LiveRow`

A `TableRow` that washes when the record behind it changes state.

```tsx
<TableBody>
  {payouts.map((payout) => (
    <LiveRow
      key={payout.id}
      washTrigger={payout.version}
      washHue={PAYOUT_STATUS[payout.state].hue}
    >
      <TableCell><IdDisplay value={payout.id} /></TableCell>
      <TableCell><PayoutPill state={payout.state} /></TableCell>
      <TableCell align="end"><Money amount={payout.amount} currency={payout.currency} /></TableCell>
    </LiveRow>
  ))}
</TableBody>
```

| Prop | Type | Notes |
|---|---|---|
| `washTrigger` | `string \| number` | **Any value that changes to trigger a wash.** Pass the row's `@version` — an exact change key, immune to clock skew. A timestamp compared against the wall clock is not. |
| `washHue` | `StatusHue` | The destination state's hue, so the wash tints toward where the row landed. Default `neutral`. |
| …plus everything `TableRow` takes | | including `selected`. |

The contract it enforces is "an in-place status change never moves a row":
the tint appears and decays, and nothing else moves or resizes. A changed
`washTrigger` starts the wash; equal values do nothing, so a re-render that
does not change the version is silent.

Under reduced motion — read live through `useReducedMotion()`, so flipping
the OS preference with the page open takes effect — the wash becomes a
**static 1200ms hold** rather than a timed decay. The signal survives; only
the animation stops. That is the rule for anything you animate here.

## Inspecting a record

### `PayloadInspector`

Raw request, response and callback bodies, verbatim, in a native `<details>`
accordion.

```tsx
<PayloadInspector
  defaultOpen={0}
  exchanges={[
    {
      direction: "request",
      method: "POST",
      url: "https://api.provider.example/v1/payouts",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ amount: 31842000, currency: "XAF", msisdn: "+237677123456" }, null, 2),
    },
    {
      direction: "response",
      status: 201,
      durationMs: 412,
      body: JSON.stringify({ reference: "OM-9F2A4C7E", status: "PENDING" }, null, 2),
    },
    {
      direction: "callback",
      status: 200,
      body: JSON.stringify({ reference: "OM-9F2A4C7E", status: "FAILED", code: "INSUFFICIENT_FLOAT" }, null, 2),
      error: "Signature verified. Provider reported a terminal failure.",
    },
  ]}
/>
```

`PayloadExchange` is `{ direction: "request" | "response" | "callback";
method?: string; url?: string; status?: number; durationMs?: number; headers?:
Record<string, string>; body?: string; error?: string }`. Each exchange gets
Body / Headers / Error tabs, the last only when `error` is set. A status
under 400 renders in neutral chrome, 400 and over in danger chrome.

| Prop | Type | Notes |
|---|---|---|
| `exchanges` | `PayloadExchange[]` | In order. Treated as an append-only log. |
| `defaultOpen` | `number` | Index to open. Default `0`; pass `-1` to start with everything collapsed. |
| `maxInlineBytes` | `number` | Bodies longer than this collapse behind an explicit "Load full payload (N KB)" action. Default `262144`. |

**No syntax highlighting, on purpose.** A rainbow JSON block would be the
loudest thing on a diagnostic screen, and on these screens colour is
reserved for state.

### `StateTimeline`

A record's transition history: one node per state entered, with the elapsed
gap between them, optional per-transition metadata, and an optional payload
inspector per node.

```tsx
<StateTimeline
  system={PAYOUT_STATUS}
  currentState="unknown"
  isTerminal={false}
  timezone="Africa/Douala"
  annotations={{
    unknown:
      "Sent, but no receipt ever arrived. It will not be resubmitted — a deliberate trade against paying twice.",
  }}
  transitions={[
    { toState: "accepted", at: "2026-09-11T09:00:00Z", actor: "api" },
    {
      toState: "submitted",
      at: "2026-09-11T09:00:01Z",
      providerKey: "orange_cm",
      attempt: 1,
      maxAttempts: 3,
    },
    { toState: "unknown", at: "2026-09-11T09:00:31Z", providerKey: "orange_cm" },
  ]}
/>
```

| Prop | Type | Notes |
|---|---|---|
| `transitions` | `StateTransition<S>[]` | `{ toState, at, actor?, providerKey?, workerNode?, attempt?, maxAttempts?, payload? }`; `at` is ISO 8601 and `payload` is `PayloadExchange[]`. An empty array renders a skeleton. |
| `system` | `StatusSystem<S>` | The same presentation table the record's `createStatusPill` is bound to, so a timeline and a pill can never disagree about what a state looks like. |
| `currentState` | one of the system's state keys | Used for the in-flight cap. |
| `isTerminal` | `boolean` | **Passed in, not derived** from the state's `family`: terminality is the server's fact, and a presentational table is the wrong place to learn it from. |
| `timezone` | `string` | Any IANA zone name. Default `"UTC"`. |
| `annotations` | `Partial<Record<S, string>>` | Per-state notes, rendered beneath the node that entered that state. |

While the record is still moving (`isTerminal={false}`) the rail continues
past the last node as a dashed segment ending in the current state's glyph
and the words "still moving", so "not finished" is readable without parsing
dates. A terminal timeline simply ends.

Timestamps come from the same formatter `TimestampDisplay` uses, so the two
cannot drift into two spellings of one stamp, and each node under the first
carries its gap from the previous one (`+412ms`, `+1m 30s`, and an em dash
if the data is malformed).

**`annotations` is where the real value is.** The point is states that look
like bugs to anyone who does not already know the product decision behind
them — "we never learned the outcome, and deliberately will not retry".
Without that sentence the operator's next move is a SQL client, which is the
outcome a timeline exists to prevent. The text is the caller's because the
explanation is domain knowledge, not presentation.

An annotation node is one of the very few places italic is correct in this
system: it is a person explaining a decision the machine made, set apart
from the emitted facts around it by more than an icon. Everything else here
— an empty state's message, a `StatTile` caption — stays upright, and the
role means something only because of that.
