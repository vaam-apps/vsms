# Changelog

## 0.1.0

First release as `@vaam-apps/ui`. Previously `@vsms/ui`, a private
workspace package.

### Generalised for release

- **The status system is parameterised.** `StatusMeta`, the glyph
  geometry and the hue classes stay here; the per-state-machine tables
  moved to the consuming application. `createStatusPill(system)` binds
  one table to a pill whose `state` prop accepts exactly that machine's
  literals, replacing three near-identical pill components that differed
  only in which table they indexed (and had already drifted: one set
  `role="img"`, one did not; one supported `interactive`, two did not).
- `StateMark` now takes a `StatusMeta` rather than one application's
  state enum. The previous meta-based export, `StateMarkFromMeta`, is
  gone — this is it, renamed.
- `StateTimeline` is generic over the state machine, takes its `system`
  and its per-state `annotations` as props, and accepts any IANA
  timezone. It previously hard-coded one application's message states,
  that application's own explanatory notes, and a two-value timezone
  union whose UTC-offset suffix was literal (`"Z"` or `"+01"`) — wrong
  for every other zone and across DST. The offset is now read from
  `Intl`.
- `MsisdnDisplay` → **`PhoneDisplay`**, with the Cameroon digit grouping
  and carrier table replaced by a `format` callback and a `tag` prop.
- `StateChip`'s tones are the shared `StatusHue` vocabulary rather than a
  private four-entry copy, so `expired` and `parked` are now reachable.
- `EncodingPreview` (GSM-7/UCS-2 SMS segment counting) was removed. It is
  application domain knowledge, not a component.
- `components/bespoke/` → `components/patterns/`.

### Fixed

- **`cn()` silently deleted custom font sizes.** `tailwind-merge`
  classifies an unrecognised `text-*` value as a colour, so
  `cn("text-caption text-state-danger-fg")` returned only the colour —
  affecting `FieldError`, `DetailList`, `CardHeader` and others, which
  had been rendering at the browser default size. The theme's font-size
  and radius scales are now registered. Present in tailwind-merge 2.6.0
  as well as 3.x; a latent bug, not an upgrade regression.
- **`cn()` did not resolve daisyUI component modifiers**, so
  `cn("btn btn-primary", "btn-ghost")` kept both and let stylesheet order
  decide. Now grouped — with `btn`/`badge` *colour* and *style* kept
  separate, because daisyUI 5's `.btn-outline` reads the variable
  `.btn-primary` sets and collapsing them discards the colour.
- **`--state-warning-*` was referenced but never declared.**
  `StateChip tone="warning"` and every `InlineBanner variant="warning"`
  (so every `StaleWriteBanner`) emitted classes matching no rule and
  rendered with no colour. Both bug classes are now build failures: see
  `lib/theme-tokens.test.ts`.
- **The copy-to-clipboard affordance leaked a timer and swallowed
  rejections.** Unmounting inside the 1.5s confirmation window called
  `setState` on a dead component, and a refused `navigator.clipboard`
  write (insecure origin, permissions policy) was an unhandled rejection
  with no user-visible result. Extracted to `CopyButton` and fixed once.

### Added

- `Money` and `formatMoney` — minor units in, `Intl`-derived exponents,
  string-based scaling that stays exact past `Number.MAX_SAFE_INTEGER`.
- `Calendar`, `DatePicker`, `DateRangePicker` on `react-day-picker`,
  exchanging `YYYY-MM-DD` strings rather than `Date` objects.
- `Checkbox` / `CheckboxField`, `Switch` / `SwitchField`, `Spinner`,
  `Progress`, `Pagination`, `ConfirmDialog`, `MaskedValue`, `StatTile`,
  `CopyButton`.

### Dependencies

- `tailwind-merge` 2.6 → 3.6 (the 3.x line targets Tailwind v4, which
  this package already required).
- `lucide-react` 0.469 → 1.44.
- `react-day-picker` 10.0 added.
- `@radix-ui/react-dialog` added as a direct dependency. It arrives
  transitively through `vaul` regardless; declaring it is what lets
  TypeScript name the drawer's re-exported types in the emitted
  declarations (TS2742).
