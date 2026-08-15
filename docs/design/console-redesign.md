# Admin console redesign — research and build plan

Status: **research and planning only.** No component code changed in this PR. This
document is the reference lock and decision ledger later build agents work from —
if a later agent's output disagrees with this document, the document wins; propose
an amendment here first, don't silently drift.

Scope: `frontends/apps/admin/` (18 route directories) and `frontends/packages/ui/` (31 existing components).
Constraints below are the maintainer's own words, verbatim where quoted, and are
non-negotiable.

---

## 0. The hard constraints, as given

1. English only — no i18n layer, no locale switching.
2. Dark theme only — no light theme, no toggle. `theme-toggle.tsx` gets deleted.
3. Side menu per a named reference screen; main container holds the business logic.
4. Drawer for more details — `vaul` (already a dependency, 1.1.2).
5. Quick details — also `vaul`. The difference between the two must be defined
   precisely, not left to feel.
6. Headless UI × TailwindCSS × DaisyUI × rounded shapes.
7. Very little custom CSS — DaisyUI does the work.
8. CVA × clsx × tailwind-merge for variants.
9. ~~`@uidotdev/usehooks` for hook needs.~~ **Withdrawn by the maintainer, 2026-08-14** — see D12. The package is not a dependency of this repo; hook needs are met by hand-written hooks in `@vsms/hooks` or by CSS where the value is a style rather than a value.
10. Mobile-first, throughout.

These override the existing design system's own prior decisions where the two
conflict (see §2, especially the radius and locale entries) — the existing system
was built to an earlier, different brief, and this one supersedes it.

---

## 1. Reference lock

### 1.1 Primary reference — LottieFiles dashboard (side menu, mandated)

**Screen:** [`eb6e3fec-1ee8-473f-b7ae-b446c5197258`](https://refero.design/pages/eb6e3fec-1ee8-473f-b7ae-b446c5197258) — `app.lottiefiles.com`, Dashboard, light theme.

This is a **structure and density** lock, not a color lock — the source is light and
this console is dark-only, so every color observation below is discarded and every
layout/grouping/density observation is kept.

Concrete structure, top to bottom:

- **A full-width top bar** sits above both the sidebar and the content, not inside
  the sidebar: wordmark (far left) → search field (wide, rounded, placeholder
  `Search Billy's Workspace`) → a promo/upgrade pill button → a help icon button →
  a notification bell icon button → an avatar-and-chevron account control (far
  right). Six items, one row, evenly spaced by function (identity / search /
  upsell / help / alerts / account).
- **A fixed-width left sidebar** (~240–260px, roughly a quarter of the viewport at
  desktop width) below the top bar, independently scrollable from the main content.
- **Primary nav is a flat, ungrouped list** at the very top of the sidebar: four
  items (`Dashboard`, `My Public Animations`, `My Collections`, `Shared with Me`),
  each icon-left + label, one row height, no section header above them — these are
  the "always visible, no grouping needed" destinations.
- **The active item is a filled rounded rectangle**, not an underline or a color
  change alone — `Dashboard` sits on a soft tinted pill spanning the full row
  width, corners visibly rounded (not sharp, not fully pill-shaped either — a
  rounded rectangle, consistent with constraint 6).
- **A workspace-switcher card** follows immediately: workspace name, a small plan
  badge (`Free`), an overflow (`···`) menu, and a `+` add action on the same row.
  This is the one place the sidebar breaks from flat rows into a bordered/tinted
  block — it reads as "context," not "navigation."
- **Two labelled, collapsible groups** come next — `PROJECTS` and `COLLECTIONS` —
  each a small-caps, muted section header with its own `+` add action on the same
  row, then 0–1 child rows beneath (icon + label, no visible nesting indent beyond
  the section's own left edge).
- **A usage/quota block**: a label (`Files uploaded`), a count (`4 / 10`), a thin
  horizontal progress bar, and an `Upgrade to upload more` text link beneath it —
  a card-like block but with no visible border, just internal padding.
- **A promo tip card**: bordered, rounded, an icon top-left, one bold line of copy,
  a `Learn more` link — the one place the sidebar carries marketing rather than
  navigation or state.
- **Footer utility rows**, visually de-emphasized (smaller, no icon-background
  fill, grouped at the very bottom of the scrollable sidebar, separated from
  everything above by a hairline): `Team space` (with a small avatar), `Create new
  workspace`, `Invite members`. These are account/workspace-lifecycle actions, not
  page destinations — they read as "administrivia," not "content."
- **Main content**: page title top-left, bold, single line, no breadcrumb; the
  content area itself is empty in this particular capture (a loading spinner,
  centered) — not useful for content-density lessons, only for confirming the
  content area gets the full remaining width with generous (~24px) outer padding.

**What is taken from this screen, specifically:**
- The **three-tier sidebar composition** (flat primary nav → labelled collapsible
  groups → de-emphasized footer utility rows), reused directly for this console's
  IA (§4).
- The **filled-rounded-rectangle active state**, reused for the console's own nav
  item styling (rounded shapes, constraint 6).
- The **top bar carrying search + account, sidebar carrying navigation** split —
  this console keeps that split rather than folding search into the sidebar.
- The **section header + inline add-action** pattern is adapted, not copied
  verbatim — this console's groups don't need a `+` (nothing is created from the
  sidebar), but the small-caps muted header with a hairline above it is kept.

**What this screen does not answer, and had to be researched elsewhere:** it shows
no dark-mode treatment, no collapse/hamburger behavior, and no state under content
weight (the content pane is empty) — see §1.2–§1.4.

### 1.2 Dark-mode sidebar-plus-table — Column.com developer dashboard

**Screen:** [`2f83156e-49d7-4a39-a988-cd4e39ba7c96`](https://refero.design/pages/2f83156e-49d7-4a39-a988-cd4e39ba7c96) — `dashboard.column.com`, API Keys, dark theme.

This is the **"what does the LottieFiles structure look like once it's actually
dark and full of operator data"** reference — the single closest analogue to what
this console needs to become, both in tone (developer/infra tool, not consumer
SaaS) and in content shape (a dense settings/keys table, a Create-record primary
button, tabs inside the content pane).

Taken from this screen:
- **The dark palette relationship**: near-black sidebar (`#121212`-class) against
  a slightly different near-black content background (`#181818`-class) — two
  distinct-but-close darks, not one flat color for the whole shell. This console's
  existing token ladder (`--color-base-100/200/300`, already three dark steps) is
  compatible with this and is kept (§2).
- **Selected-nav-row treatment in dark mode**: a solid blue fill with white text,
  confirming a filled rounded rectangle (not just a lighter-gray tint) survives
  the transition to dark and stays legible — validates §1.1's active-state choice
  in a dark context specifically.
- **A horizontal tab bar living inside the content pane**, below a page-level
  title/breadcrumb row, for switching between sibling views of one resource
  (`Info` / `Settings` / `API Keys` / `Webhooks` / `Root Entity` / `Sandbox`) —
  this is the pattern for e.g. a Provider or Sender ID detail surface that has
  more than one facet, kept distinct from the sidebar's own page-level nav.
- **Table conventions**: partially-obscured secret values with an inline copy
  icon (directly reusable for `WebhookEndpoint.secret`), a primary "Create X"
  button top-right of the table header, row dividers with no zebra striping.

### 1.3 Dark three-column workspace — Linear (project overview)

**Screens:** [`7e8f91a0`](https://refero.design/pages/7e8f91a0-fb3d-4db2-bf2b-a69809717c8a), [`b070f7ac`](https://refero.design/pages/b070f7ac-aa07-47e0-98dc-bc27f8a8cc42) — `linear.app`, dark theme.

Taken: confirmation that a narrow icon-first sidebar (Linear collapses its own
labels at the width this console's sidebar would use on a laptop) is a legitimate
dark-mode density move, feeding the collapse behavior decision in §2.

**Explicitly rejected, not taken:** Linear's third column (a persistent right-hand
detail/property panel, always visible, distinct from an on-demand drawer). The
maintainer's constraint is "side menu + main container" — two regions, not three.
A record's details in this console live in a `vaul` drawer that opens over the
main container, never in a third permanent column. Naming this rejection
explicitly matters because Linear is an easy screen to over-borrow from.

### 1.4 Dense table + slide-over quick view — Mercury (transactions, cards)

**Screens:** [`e8da26fb`](https://refero.design/pages/e8da26fb-1dba-4eba-a545-125f962e2fb8) (transactions table), [`fa1ce2ef`](https://refero.design/pages/fa1ce2ef-fdb7-4311-90da-6efb429cf8f5) (transaction + right drawer + comments), [`f3e9c6d3`](https://refero.design/pages/f3e9c6d3-d1c9-4b62-a198-cf4ee81b9ebb) (card list + right detail sheet) — `demo.mercury.com`, light theme, finance/ops product.

Taken (structure only, not the light palette):
- **A dense, multi-column, filterable table is the primary content shape**, with a
  summary/metric strip above it, not the exception — directly analogous to this
  console's Messages/Jobs/Providers tables.
- **Clicking a row opens a right-side drawer over the (undimmed-enough-to-stay-
  legible) table**, not a route change, not a full-page navigation, for a
  transaction/card's own detail — this is the concrete precedent for "quick
  details" (§3): a summary of the clicked record plus a small number of actions,
  while the list underneath stays exactly where it was, scrollable and unblurred.
- **The drawer is genuinely narrower than a full page** — roughly a third of the
  viewport at desktop width, never full-bleed, reinforcing that a quick-detail
  drawer should feel like a peek, not a destination.

### 1.5 Drawer-as-form-surface — Polar.sh (dark, product/benefit creation)

**Screens:** [`8d76b115`](https://refero.design/pages/8d76b115-5345-4212-b02f-9e01859794f7), [`c0ae70e7`](https://refero.design/pages/c0ae70e7-6132-474d-9b0b-55f6723e6148), [`ad269293`](https://refero.design/pages/ad269293-5695-4343-b9f5-8c8240e5ebee) — `polar.sh`, dark theme, dashboard admin.

This is the reference for the **other** use of a drawer in this system — not a
read-only quick peek, but a genuine create/edit form living in a drawer rather
than a page or a centered dialog. Taken:
- A right-side drawer, wider than Mercury's quick-view drawer (closer to half the
  viewport), holding a single-column stacked form (name field, grouped condition
  cards, dropdowns, a toggle) with a fixed bottom action row (`Cancel` / primary
  create action) that stays visible without the form needing to scroll to reach it.
- The main workspace behind the drawer is visibly dimmed (an overlay), unlike
  Mercury's quick-view drawer which left the table legible — signaling "this is a
  modal-weight interaction," even though it's still a slide-over, not a centered
  dialog. This is the concrete evidence for why "more details" (§3) dims the
  background and "quick details" does not.

### 1.6 Empty states inside a live table — Fingerprint, Gladia, Twitch

**Screens:** [`65979144`](https://refero.design/pages/65979144-0975-45fd-9396-8c1f6b6243b1) (Fingerprint, pending invites), [`b8dc0f32`](https://refero.design/pages/b8dc0f32-e2c7-46da-a860-fcf3597a4518) (Gladia, transcriptions), [`f73c4fa4`](https://refero.design/pages/f73c4fa4-c957-4ae1-b96b-34a5b405cc7d) (Twitch, roles).

Taken: an empty state renders **inside the table's own bordered region**, at the
row position where content would be, with the filter/toolbar chrome still fully
present above it — never a full-screen illustrated placard replacing the whole
page. This matches (and confirms as correct, not a local quirk) this console's
existing `InlineEmptyState` component and its own house rule (§2.0 of the CLAUDE.md
design-philosophy section: "empty states are inline status lines, not centered
placards unless the screen has nothing else to do").

### 1.7 Destructive confirmation with a delay fuse — Jace AI, Gladia, Cursor

**Screens:** [`263275ec`](https://refero.design/pages/263275ec-a055-459b-a70c-1e69e2f2ea40) / [`b7c8625a`](https://refero.design/pages/b7c8625a-ce01-4870-bba2-3353967616e4) (Jace AI, delete account), [`ffde0be1`](https://refero.design/pages/ffde0be1-879a-4b99-92dd-8d939deafc10) (Gladia, delete API key, typed confirmation), [`4f580aa3`](https://refero.design/pages/4f580aa3-0748-4e78-ae3d-95780cb2c523) (Cursor, delete account, typed confirmation).

Taken: a **centered dialog**, not a drawer, for anything irreversible or scary —
warning copy above the fold, an optional typed-confirmation field for the
highest-stakes actions, two right-aligned buttons (`Cancel` neutral, the
destructive action in the danger hue, never the primary/accent hue). This is the
concrete precedent locking "destructive confirmation = centered `Dialog`" into
§3's drawer-vs-dialog-vs-page rule.

**Correction, found live while building the Routes/Webhooks/Sender IDs screens:
a centered `Dialog` opened from *inside* an already-open drawer does not work
with this stack, at all.** The reference screens above (Jace AI, Gladia,
Cursor) all show the confirmation as a *page-level* action — there is no
enclosing drawer in any of them. This document's original text extrapolated
that pattern to "confirmations nested inside `MoreDetailDrawer`" (§3's own
"secret rotation's own confirmation step inside that flow stays a nested
`Dialog`" line) without checking whether the primitives actually support it.
They don't: `MoreDetailDrawer`/`QuickDetailDrawer` (`vaul`) always mount a
trapped, document-level `@radix-ui/react-focus-scope` `FocusScope` — regardless
of `dimmed`, since `vaul` never forwards its own `modal` prop down to
`@radix-ui/react-dialog`'s `Root` — and Headless UI's `Dialog` always portals to
a *sibling* `#headlessui-portal-root`, outside that trap's own container. The
trap force-refocuses back into the drawer the instant the nested `Dialog` tries
to move focus into itself, permanently stalling its enter transition: the
confirmation becomes a stuck, invisible, non-interactive ghost. See
`frontends/apps/admin/app/gallery/page.tsx`'s
`NestedDialogInDrawerRegression` for the full investigation (four
primitive-level fixes tried, all reproduced the bug; one hit a second, real
`vaul@1.1.2` bug of its own) and §3 below for the corrected rule. A destructive
confirmation triggered from a screen with **no** enclosing drawer open at the
time — the route-simulator's own confirm, and anything else that is a
page-level action rather than a step inside a drawer flow — is unaffected and
stays a real, centered `Dialog`; the bug is specific to nesting one inside an
already-open `vaul` drawer.

---

## 2. Decision ledger

Every real choice below states the alternative considered and why it lost. Where
an existing `frontends/packages/ui` decision conflicts with a new constraint, that is called
out explicitly — the new constraint always wins, but the reasoning for the old
choice is recorded so nobody re-litigates it from scratch.

| # | Decision | Alternative considered | Why it lost | Source |
|---|----------|------------------------|-------------|--------|
| D1 | Side menu structure follows LottieFiles' three-tier composition (flat primary nav → labelled collapsible groups → de-emphasized footer utility) | A flat, single-level list of all 18 routes | Explicitly named in the task as "not a design"; also fails the reference lock, which shows grouping as load-bearing | §1.1, §4 |
| D2 | No third (right-hand) permanent panel; record detail lives only in a `vaul` drawer over the main container | Linear-style persistent right property panel | Maintainer's constraint is exactly two regions (side menu + main container) | §1.3, constraint 3 |
| D3 | Radix → Headless UI for every primitive that has a Headless UI equivalent (Dialog, Menu, Listbox, Popover, Tab, Field/Label, Disclosure) | Keep Radix, since it works | Explicit constraint 6; Headless UI is unambiguously named as the behavior layer | §5 (inventory) |
| D4 | `@radix-ui/react-slot` and the `asChild` prop are **removed**, replaced by exporting a standalone `buttonVariants(...)` CVA function so link-as-button composition doesn't need a polymorphic wrapper | Keep Radix Slot as a targeted, retained exception (it has no behavior, only structural cloning) | Headless UI has no Slot equivalent and none is needed — CVA already gives every consumer the class string directly; keeping one Radix package alive for a non-behavioral utility contradicts "very little custom CSS / lean on the stack" | §5, §7 |
| D5 | `@radix-ui/react-tooltip` is replaced by DaisyUI's native `.tooltip`/`data-tip` CSS component | Hand-roll a Headless-UI-adjacent tooltip (e.g. via `@floating-ui/react`) | Headless UI ships no Tooltip. Introducing a new dependency to replace one Radix package with another behavior library contradicts constraints 6–7; DaisyUI's CSS-only tooltip is hover/focus-`title`-driven, no portal, no JS — accepted limitation: no rich/interactive tooltip content anywhere in this console (nothing currently needs one — richer content already renders inline, e.g. `PayloadInspector`, `StateTimeline` annotations) | §5, §7 (risk) |
| D6 | `@radix-ui/react-separator` is deleted outright, replaced by a 3-line plain `<div role="separator">` styled with a DaisyUI-token border color | Keep Radix Separator | It is already a near-trivial wrapper (`SeparatorPrimitive.Root` renders a styled div); zero behavior is lost by dropping it | §5 |
| D7 | Sidebar responsive collapse uses **DaisyUI's own `.drawer` CSS component** (checkbox-driven off-canvas container, no JS state) for the mobile/tablet nav, kept semantically and *nominally* distinct from `vaul`'s `Drawer` (used only for record detail panels) | Reuse `vaul` for the off-canvas sidebar too, since "drawer" is already the vocabulary | These are two different concerns with an unfortunate name collision: `vaul`'s `Drawer` is a JS-driven, swipeable, portal-rendered overlay for *content* (detail panels); DaisyUI's `.drawer` is a pure-CSS, checkbox-driven, non-portal off-canvas *layout* container purpose-built for exactly "sidebar that becomes an overlay below a breakpoint." Using `vaul` for navigation chrome would add JS overhead and swipe-gesture behavior nothing asks for, and using DaisyUI's `.drawer` for record detail would fight `vaul`'s own portal/animation model. The new nav component is named `ConsoleShell`/`SideNav`, never "Drawer," specifically so later agents don't reach for the wrong one by name association | §4, §6 |
| D8 | Radius scale is **rewritten from the existing near-square 2px system to a confidently rounded one**: `--radius-selector: 8px`, `--radius-field: 12px`, `--radius-box: 20px`; Tailwind's own `--radius-sm`/`--radius-md` aliases move from `2px`/`4px` to `12px`/`20px` to match; `--radius-full` (9999px) stays available for true pills | A moderate register (an earlier draft of this document picked `6/8/12px` specifically to stay close to the outgoing 2px system) | Judged on the reference lock alone, not against what shipped before: LottieFiles' filled-rounded-rectangle active nav row, Column's selected-row fill, and Polar's drawer/card corners (§1.1, §1.2, §1.5) all read as genuinely, confidently rounded — not a restrained nudge off square. This project's own standing convention is a hard cutover, not a cautious half-step, when replacing something ("we're breaking what exists" — maintainer, on this exact decision): "it was already built carefully" is not a reason to hedge the replacement. The one thing the register still respects is the reference lock's own ceiling, not the old system's: §1.1 is explicit that even LottieFiles' own active row is "not fully pill-shaped", and a data table's chrome would look novelty-bubbly at true pill radius — so this stays short of `--radius-full`, but at the confident end of what's short of it, not the cautious end | §1.1, §1.2, §1.5, §2 (theme.css) |
| D9 | Dark-only theming: **delete** the `"light"` `@plugin "daisyui/theme"` block and its paired `[data-theme="light"]` CSS block from `theme.css` entirely; delete `theme-toggle.tsx`; remove every `<ThemeToggle />` usage; `frontends/apps/admin/app/layout.tsx` keeps `<html data-theme="dark">` (harmless, avoids any `[data-theme]`-selector edge case in DaisyUI internals, and documents intent) even though only one theme exists | Keep both themes defined but simply never expose a toggle in the UI | Constraint 2 says "no light theme" — leaving dead light-theme tokens in the codebase is exactly the kind of dormant code the project's own delivery-style rule forbids ("never implement something and leave it dormant"); a maintained second theme nobody can reach is a liability, not a convenience | Constraint 2, project CLAUDE.md "Delivery style" |
| D10 | English-only: **`labelFr`/`tooltipFr` are deleted outright** from `status-tokens.ts`, and the `locale` **prop is removed** from every UI component's public API (`StatusPill`, `StateTimeline`, transitively `JobStatusPill`/`AttemptStatusPill`). Every call site renders English, unconditionally | Keep the French data as unused fields, on the theory a future CSV export or compliance report might want them | **Settled by the maintainer, 2026-08-14: delete them.** This document originally inferred "keep the data, drop the switch" and flagged the question; the answer is the stronger reading of the constraint. Retaining localisation data that nothing renders is the same "claims a capability that does not exist" smell this repo has repeatedly found and fixed (an unenforced permission literal, an event type nothing could emit) — and a future consumer that genuinely needs French can add it deliberately, against a real requirement, rather than inheriting a half-maintained table nothing exercises. **Deleting the fields is what makes the removal verifiable**: with them gone, any surviving reader is a compile error rather than silently-dead code. **Not an abandonment of localisation** — [#231](https://github.com/vymalo/vsms/issues/231), filed after this decision, tracks a real `react-i18next` layer for the console, still English-only by default with additional languages opted into via env-var config; that is the actual mechanism this decision defers to, not two ad hoc string fields on a domain-semantics table | Constraint 1 |
| D11 | `cva()` replaces the existing hand-written `Record<Variant, string>` variant maps (`Button`'s `VARIANT_CLASSES`/`SIZE_CLASSES`, `Badge`'s inline ternaries) | Keep the `Record`-based maps, since `clsx`+`tailwind-merge` already work today | Constraint 8 names CVA specifically, not just "some variant mechanism" — and `cva` gives `compoundVariants` for states the `Record` approach can't express cleanly (e.g. a future `loading` + `size=icon` combination). Migration is mechanical and must preserve byte-identical class output, verified before merge (see §6, §7) | Constraint 8 |
| D12 | **`@uidotdev/usehooks` is not a dependency of this repo — the constraint was withdrawn by the maintainer on 2026-08-14, after Phase 0.** Two findings drove it. First, the package's `useMediaQuery` has a `getServerSnapshot` that is a hard `throw new Error("useMediaQuery is a client-only hook")` (read directly from `@uidotdev/usehooks/index.js`; the only hook in the library built that way, confirmed by grepping every `getServerSnapshot` in it) — that 500'd every full page load in `SideNav` while `pnpm build` stayed green, because every route here is dynamic and build-time generation never executes them. Second, its last real publish was **2023-10-23** (verified against npm's own `time` map — note `time.modified` reads 2026-05-14 and is a metadata touch, not a release), so it predates React 19 entirely, which this repo is on. **Breakpoints are CSS-first**: the sidebar's three bands (§6.1) and the drawer's mobile/desktop weights (§6.4) are plain Tailwind responsive classes — no hook, no client-only render boundary, no SSR trap. Where a value genuinely must be *read in JS* and cannot be expressed as a style, write the hook by hand in `@vsms/hooks` and give it an explicit SSR-safe default. `LiveRow`'s `prefers-reduced-motion` check is the one confirmed such case (it feeds a numeric `setTimeout` duration, which no CSS query can drive) and its **existing** hand-rolled check is already SSR-safe by its own shape: `typeof window !== "undefined" && window.matchMedia(...).matches`, evaluated at render time rather than stored in state, defaulting to "motion allowed" on the server and re-evaluating identically on the client's first render. Leave it exactly as it is | Adopt the package as originally constrained; or keep it for the handful of generic hooks it does provide | An unmaintained dependency that predates the React major this repo runs on, whose headline hook is actively hostile to server rendering, is not worth carrying for three generic hooks that are a few lines each to write. After Phase 0's CSS-first rework it had **zero live imports** — a declared dependency with no call sites — which made dropping it free rather than a migration | Maintainer, superseding constraint 9 |
| D13 | **Not** replaced by `usehooks`, and must not be: `TimestampDisplay`'s shared 30-second-tick external store (a deliberate single-timer-for-N-instances optimization, not a generic hook), and `messages-screen.tsx`'s self-scheduling long-poll loop (`utils.client.messages.onStateChange.query(...)` inside a manually-managed `while` loop) — both encode product-specific correctness properties, not generic hook patterns | Force everything hook-shaped through `usehooks` for consistency | `usehooks` has no long-poll-with-backpressure primitive and no shared-timer-across-instances primitive; forcing these onto a generic hook would either lose the "one timer, N subscribers" property or reintroduce the exact `refetchInterval`-stalls-after-two-calls bug already found and fixed live (see AGENTS.md's M3 messages-screen section) | AGENTS.md (messages-screen module doc), §7 (risk) |
| D14 | The drawer-vs-quick-detail distinction (constraint 5) is resolved as: **"quick details"** = narrow (`max-w-[420px]`–`480px`) `vaul` drawer, undimmed/lightly-dimmed background, opens from a table row, shows a summary subset + 1–2 actions, no route change, no deep-link; **"more details"** = wide (`max-w-[640px]`–`720px`) `vaul` drawer, dimmed background, owns a shallow route (`?panel=<id>`) so it's linkable and survives refresh, holds the full record plus an edit form and destructive actions | A single drawer component with a `size` prop and no other distinction | The maintainer explicitly asked for the difference to be *precise*, not a size tweak — dimming, route ownership, and content depth all correlate with "is this a peek or a destination," and conflating them (e.g. a wide undimmed drawer, or a narrow deep-linkable one) would produce exactly the "two drawer usages that feel the same" failure named in the task | Constraint 5, §1.4, §1.5, §3 |
| D15 | Message detail (`/messages/[id]`) **stays a full page route**, not a drawer of either kind | Fold it into a "more details" drawer, since it is a per-record detail view like every other one | It already exceeds even a wide drawer's comfortable depth (full state timeline, raw payload inspector with per-exchange tabs, delivery-receipt list) and is the console's primary investigative destination, not a peek from a list — the rule that falls out of this (§3) is "if closing it should feel like leaving a page, it's a page" | §1.4 (Mercury contrast), existing `message-detail-screen.tsx` |
| D16 | The `Table` primitive adopts DaisyUI's real `table`/`table-pin-rows` classes underneath the existing bespoke row/cell behavior (no zebra, `LiveRow` wash, mono cells), rather than staying pure hand-written Tailwind utility classes | Leave `Table` as-is — it already looks right | Constraint 7 ("DaisyUI does the work") is not satisfied by a component that never references a single DaisyUI class; today's `Table` is 100% Tailwind utilities reimplementing what `table`/`table-pin-rows` already provide | §5, constraint 7 |
| D17 | Select uses Headless UI's `Listbox` (closed enumerated choice, e.g. state filter) for existing `Select` call sites; `cmdk`'s `CommandMenu` stays exactly as-is (fuzzy search / command palette, unrelated to Radix, no migration needed) | Migrate everything, including `cmdk` usage, "to be consistent" | `cmdk` was never a Radix component and constraint 6 only names Headless UI as the Radix replacement — `cmdk` already satisfies "standalone, owns its own keyboard nav/ARIA" the way the original T6 brief wanted; touching it is unnecessary churn | §5 |
| D18 | Tabs: existing call sites use a **value-based, controlled API** (`Tabs value=/onValueChange=`, matching Radix); Headless UI's `TabGroup` is **index-based** (`selectedIndex`/`onChange(index)`). A small adapter component (`ValueTabs`) is built once, wrapping `TabGroup`, so `PayloadInspector`, `message-detail-screen.tsx`, `providers-screen.tsx`, and `webhooks-screen.tsx` need **zero call-site changes** beyond the import path | Rewrite every Tabs call site to index-based state | The value-based API is more resistant to bugs when tab order changes and is already threaded through four files; a thin, one-time adapter is far cheaper than four rewrites plus their tests, and keeps the public `@vsms/ui` API stable | §5, §6 (build order) |

---

## 3. Drawer vs. quick details vs. dialog vs. page — the actual rule

This is the single most load-bearing decision in this document (constraint 5 asks
for it explicitly), stated as one rule so every future screen answers it the same
way:

> **Dialog** (centered, modal, Headless UI `Dialog`) — an action that needs a yes/no
> answer before anything else can happen, triggered from a screen with **no
> drawer already open**: destructive confirmations (delete, revoke,
> rotate-with-consequence), and short single-purpose forms with no
> sub-navigation (rename, confirm-requeue, "New X" from a toolbar). Always dims
> the background. Never scrolls the page behind it. §1.7.
>
> **Inline confirmation** (`@vsms/ui`'s `InlineConfirm`, rendered as the
> drawer's own body/footer, no portal) — the *same* two shapes as `Dialog`
> above (yes/no confirm, or a short form with one or two fields), but
> triggered from *inside* an already-open `QuickDetailDrawer`/
> `MoreDetailDrawer`. **Never a nested `Dialog` for this case** — see §1.7's
> own correction: a centered `Dialog` opened while a `vaul` drawer is open
> never becomes visible, a real bug in this exact library combination, not a
> style preference. The caller swaps the drawer's body (and its `footer`
> prop, since `InlineConfirm` supplies its own Cancel/Confirm row) rather
> than layering a second overlay on top.
>
> **Quick details** (narrow `vaul` drawer, `direction="right"` desktop /
> `"bottom"` mobile, `max-w-420px`–`480px`) — a peek at one row's state without
> leaving the list: a summary of the record's own fields already visible or one
> fetch away, 1–2 actions, closes back to exactly where the list was scrolled to.
> Background stays legible (light or no dim). No route ownership — refresh loses
> it, and that's fine, because reopening it is one click on the same row. §1.4.
>
> **More details** (wide `vaul` drawer, `direction="right"` desktop / `"bottom"`
> mobile at near-full height, `max-w-640px`–`720px`) — the full record: every
> field, an edit form, destructive actions, nested history if it's short. Dims the
> background (this is modal-weight, just not centered). Owns a shallow route
> (`?panel=<recordId>`) so it survives refresh and is linkable/shareable. §1.5.
>
> **Page** — the record has enough of its own internal structure (its own
> timeline, its own tabs, its own large data tables) that even a wide drawer would
> cramp it, or it is the console's primary destination for that entity, not a
> detail of something else. Message detail is the one existing example (§1.4,
> D15). The test: *if closing it should feel like leaving a page, it's a page; if
> closing it should feel like dismissing a panel, it's a drawer.*

Concretely, per screen (§4's groups):
- **Quick details**: a Providers/Routes/Sender IDs/Jobs/Opt-outs table row →
  narrow drawer with the row's own fields + a "View full details" link that
  upgrades to the wide drawer.
- **More details**: Provider edit, Route edit, Sender ID registration
  review/approve-reject, Webhook endpoint edit + secret reveal/rotate — the
  existing `providers-screen.tsx` edit `Dialog` (§1.7's territory is
  confirmations, not multi-field edit forms) becomes a **more-details drawer**,
  matching D14/D18's reasoning; secret rotation's own confirmation step inside
  that flow is an **inline confirmation** (see the rule above), not a nested
  `Dialog` — corrected from this document's original text, which specified a
  nested `Dialog` here without having checked it against the actual
  primitives; see §1.7's own correction note for what was found and fixed.
- **Page**: Messages list + Message detail (existing), Dashboard, Simulator,
  Audit log (a page-scale table, not a record of something else), Settings.

On mobile (<768px), both drawer weights render `direction="bottom"`, height
capped at roughly 90vh with a drag handle — "quick" stays visually shorter
(content-sized, not full height) while "more" opens closer to full height,
preserving the same weight distinction the desktop side-drawer widths express.

---

## 4. Information architecture

18 routes are not 18 flat sidebar rows. Grouped per §1.1's three-tier composition:

```
[ Dashboard ]                                    ← flat, ungrouped, always first

MESSAGING
  Composer            (/)
  Messages            (/messages)
  Simulator           (/simulator)

DELIVERY
  Providers           (/providers)
  Routes               (/routes)
  Sender IDs           (/sender-ids)
  Webhooks             (/webhooks)

OPERATIONS
  Jobs                 (/jobs)
  Workers               (/workers)
  Opt-outs              (/opt-outs)

ADMIN
  Apps                  (/apps)
  Users & roles         (/users)
  Audit log             (/audit-log)
  Settings              (/settings)

──────────────────────────────────────           ← hairline, footer zone (LottieFiles §1.1)
  Component gallery     (/gallery)  — dev-only, de-emphasized like "Invite members"
  Signed in as <email> · Sign out
```

`/login` is not a nav item (pre-auth gate, no shell). `/api/*` is not a page.

Rationale for the four group names: they follow the operator's own mental model,
not the schema — "MESSAGING" is what you touch to send and watch traffic;
"DELIVERY" is the infrastructure that decides how a message leaves (this is where
grey-route detection, failover, and the routing simulator's siblings live
conceptually, even though Simulator itself sits under Messaging because it's a
read-only "what would happen" tool, not delivery configuration); "OPERATIONS" is
the worker/job/opt-out machinery that keeps the system healthy; "ADMIN" is
account/access/compliance surface. Four groups matches LottieFiles' own count
(workspace/projects/collections/footer) closely enough to trust the density.

**Off-canvas, <1024px (D7) — phone and tablet alike:** the same four groups
render as DaisyUI-native accordions inside the off-canvas panel — only the
group containing the current route starts expanded; the other three start
collapsed, so a phone user sees roughly 6–8 tappable rows on open, not 18.
Tapping a group header toggles it (no animation budget spent beyond DaisyUI's
own collapse transition); tapping a route closes the off-canvas panel and
navigates. The footer zone (gallery + account) stays pinned below the
accordion list, matching desktop.

**Icon-only rail, 1024–1279px:** sidebar renders with labels hidden, tooltips
via D5's DaisyUI `.tooltip` on hover/focus, matching the Linear-derived
density note in §1.3 — full labels return at ≥1280px. **Corrected during
Phase 0 (D12):** this is a plain `lg:`/`xl:` Tailwind responsive-class
switch, not a `usehooks` `useMediaQuery` read — see D12's own updated entry
for why a JS breakpoint read was tried first and found to break SSR.

**Correction, found landing Phase 0:** the two paragraphs above used to say
"Mobile (<768px)" / "Tablet (768–1024px)", which contradicts §6.1's own
breakpoint table below (off-canvas through `md`, i.e. through 1023px; the
icon rail is `lg`, 1024–1279px) — §6.1 is the later, more carefully reasoned
version (it states its own reason: "a 768px-wide iPad-mini-class viewport is
still too narrow for a persistent rail"), and is the one this build actually
implements. The numbers above are corrected to match it rather than treated
as two independently-true breakpoint schemes.

---

## 5. Component inventory

Verdict legend: **Port** = same component, Headless UI/DaisyUI internals, same
public API. **Refresh** = same component, no Radix dependency to remove, gets the
CVA/radius/DaisyUI-class pass. **Rebuild** = public API or mechanism changes.
**Delete** = removed outright.

| Component | Current basis | Verdict | Target |
|---|---|---|---|
| `primitives/button.tsx` | Radix `Slot` (asChild) + hand `Record` variants | Rebuild | `cva()` variants; `Slot`/`asChild` removed, `buttonVariants` exported standalone (D4, D11) |
| `primitives/dialog.tsx` | Radix `Dialog` | Port | Headless UI `Dialog`/`DialogPanel`/`DialogTitle`/`DialogBackdrop` |
| `primitives/drawer.tsx` | `vaul` | Refresh | `vaul`, unchanged dependency; gains `QuickDetailDrawer`/`MoreDetailDrawer` wrapper variants (§3, D14) — direction/width/dim baked in per variant so call sites can't accidentally blur the two |
| `primitives/dropdown-menu.tsx` | Radix `DropdownMenu` | Port | Headless UI `Menu`/`MenuButton`/`MenuItems`/`MenuItem` |
| `primitives/label.tsx` | Radix `Label` | Port | Headless UI `Field`/`Label` (v2's Field composition) |
| `primitives/popover.tsx` | Radix `Popover` | Port | Headless UI `Popover`/`PopoverButton`/`PopoverPanel` |
| `primitives/select.tsx` | Radix `Select` | Rebuild | Headless UI `Listbox`/`ListboxButton`/`ListboxOptions`/`ListboxOption` (D17) — API shape differs enough from Radix Select to need call-site review, not just an import swap |
| `primitives/separator.tsx` | Radix `Separator` | Delete → replace | 3-line plain `<div role="separator">`, no package (D6) |
| `primitives/tabs.tsx` | Radix `Tabs` | Rebuild | Headless UI `TabGroup` behind a value-based `ValueTabs` adapter (D18) |
| `primitives/tooltip.tsx` | Radix `Tooltip` | Delete → replace | DaisyUI `.tooltip`/`data-tip` (D5) |
| `primitives/command-menu.tsx` | `cmdk` (standalone) | Port, unchanged | `cmdk` (D17) |
| `primitives/badge.tsx` | Hand Tailwind + DaisyUI `badge` class | Refresh | `cva()` variants, new radius scale |
| `primitives/card.tsx` | Hand Tailwind + DaisyUI `card` class (shadow opted out) | Refresh | Same shape, new radius (`--radius-box`) |
| `primitives/input.tsx` | DaisyUI `input input-bordered` | Refresh | Same, new radius via `--radius-field` |
| `primitives/textarea.tsx` | DaisyUI `textarea textarea-bordered` | Refresh | Same, new radius |
| `primitives/table.tsx` | Hand Tailwind, zero DaisyUI classes | Rebuild | DaisyUI `table`/`table-pin-rows` underneath existing bespoke row/cell logic (D16) |
| `primitives/skeleton.tsx` | Hand Tailwind | Refresh | Unchanged behavior (static, no shimmer — a house rule, not negotiable), new radius token |
| `primitives/toast.tsx` | Hand-rolled store, no Radix | Refresh | Unchanged mechanism; class pass for radius/tokens only |
| `primitives/theme-toggle.tsx` | Hand `useState` + `data-theme` flip | **Delete** | No replacement (constraint 2, D9) |
| `data/id-display.tsx` | Hand, no Radix | Refresh | Class pass only |
| `data/msisdn-display.tsx` | Hand, no Radix | Refresh | Class pass only |
| `data/timestamp-display.tsx` | Hand, shared-tick external store | Refresh (mechanism kept, D13) | Class pass only — the shared timer is domain logic, not chrome |
| `bespoke/state-timeline.tsx` | Hand, composes `StateMark`/`PayloadInspector`/`Skeleton` | Refresh | Class pass; drop `locale` prop (D10) |
| `bespoke/payload-inspector.tsx` | Native `<details>` + `Tabs` | Refresh + inherits Tabs rebuild | Class pass; `Tabs` import swaps to `ValueTabs` |
| `bespoke/encoding-preview.tsx` | Hand, composes `Textarea`/`Button` | Refresh | Class pass only — this is pure domain UI (GSM-7/UCS-2 preview), not chrome; keep as-is behaviorally |
| `bespoke/live-row.tsx` | Hand, composes `TableRow`, `matchMedia` reduced-motion check | Refresh (mechanism kept) | Reduced-motion check **stays exactly as written** — it is already SSR-safe and there is no library to migrate it to (D12); wash behavior, timing, and the "in-place, never reflow" contract are unchanged — this encodes a real correctness property (§6.5 rule 3 in the existing design doc lineage), not chrome |
| `bespoke/inline-empty-state.tsx` | Hand, no dependency | Refresh | Class pass only — the "inline, not a placard" rule is confirmed correct by research (§1.6), not revisited |
| `status/status-tokens.ts` | Pure data + `locale` prop plumbing | Rebuild (narrow) | `MessageState`/`JobState`/`AttemptState` tables, hues, and glyph choices are **completely unchanged** — this is the one file the redesign must not touch semantically. The `locale`-prop plumbing is removed from the *components* that read this file, and `labelFr`/`tooltipFr` are deleted from the tables themselves (D10) — English `label`/`tooltip`, hues, families, glyphs and attention levels are untouched |
| `status/state-mark.tsx` | Pure SVG, no dependency | Port, unchanged | Zero changes — the eleven-glyph geometry is a correctness artifact (§7) |
| `status/status-pill.tsx` | Hand, composes `StateMark` | Refresh | Class pass + drop `locale` prop; hue/attention logic unchanged |
| `status/job-status-pill.tsx` | Hand | Refresh | Same treatment as `status-pill.tsx` |
| `status/attempt-status-pill.tsx` | Hand | Refresh | Same treatment as `status-pill.tsx` |

Package consequence: `@radix-ui/react-dialog`, `-dropdown-menu`, `-label`,
`-popover`, `-select`, `-tabs` are replaced by `@headlessui/react`.
`@radix-ui/react-separator`, `-slot`, `-tooltip` are removed with no replacement
package. `cva` is added; `@uidotdev/usehooks` is **not** (D12). `clsx`, `tailwind-merge`,
`cmdk`, `vaul`, `lucide-react` are unchanged.

---

## 6. Layout system

### 6.1 Breakpoints (mobile-first — base styles target the smallest viewport)

| Token | Width | Sidebar behavior |
|---|---|---|
| base | `<768px` | Off-canvas, hidden by default, hamburger in a slim sticky top bar |
| `md` | `≥768px` | Off-canvas still (tablet keeps the hamburger — a 768px-wide iPad-mini-class viewport is still too narrow for a persistent rail per Linear's own density cue, §1.3) |
| `lg` | `≥1024px` | Persistent icon-only rail (labels hidden, `.tooltip` on hover) |
| `xl` | `≥1280px` | Persistent full sidebar with labels (LottieFiles' own ~240–260px width) |

Main container content max-width stays `1400px` (existing `messages-screen.tsx`
convention, kept — it already reads correctly against the reference density).

### 6.2 App shell sketch

Illustrative only — locks the composition, not final class names. `ConsoleShell`
wraps every authenticated route; `frontends/apps/admin/app/layout.tsx` renders it once.

```tsx
// frontends/apps/admin/app/console-shell.tsx (new; replaces console-nav.tsx's per-screen
// header pattern — see §7, "must survive untouched" for what this must NOT
// change about data fetching)
"use client";

import { NAV_GROUPS } from "./nav-groups"; // §4's structure, as data
import { SideNav } from "@vsms/ui"; // DaisyUI .drawer-driven, D7

export function ConsoleShell({ children }: { children: React.ReactNode }) {
  return (
    <div className="drawer lg:drawer-open">
      <input id="console-nav" type="checkbox" className="drawer-toggle" />

      <div className="drawer-content flex min-h-dvh flex-col">
        {/* Slim sticky top bar — visible at every breakpoint; carries the
            hamburger below `lg`, search always, account control always. */}
        <header className="sticky top-0 z-30 flex h-14 items-center gap-3 border-edge border-b bg-base-200 px-4">
          <label htmlFor="console-nav" className="btn btn-square btn-ghost lg:hidden" aria-label="Open navigation">
            {/* hamburger icon */}
          </label>
          <span className="font-mono text-caption text-subtle-foreground">vsms</span>
          {/* search, account control */}
        </header>

        <main className="flex-1 px-4 py-6 lg:px-8 lg:py-10">{children}</main>
      </div>

      <div className="drawer-side z-40">
        <label htmlFor="console-nav" className="drawer-overlay" />
        <SideNav groups={NAV_GROUPS} className="min-h-dvh w-[260px] lg:w-[64px] xl:w-[260px]" />
      </div>
    </div>
  );
}
```

`SideNav` itself owns the collapsible-group-on-mobile behavior (Headless UI
`Disclosure`, §4) and the icon-only-rail-at-`lg` behavior (**corrected
during Phase 0**: plain `lg:`/`xl:` Tailwind responsive classes, not
`usehooks`' `useMediaQuery` — D12) — kept out of `ConsoleShell` so the shell
stays a pure layout container, matching constraint 3's "main container
holds the business logic" (i.e., the shell holds none).

### 6.3 One CVA variant sketch — `buttonVariants`

Locks the exact shape D4/D11 describe: a standalone exported variant function,
`Button` built on top of it, no `asChild`.

```tsx
// frontends/packages/ui/src/components/primitives/button.tsx
import { cva, type VariantProps } from "class-variance-authority";
import type { ButtonHTMLAttributes } from "react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

export const buttonVariants = cva("btn font-sans font-semibold rounded-field", {
  variants: {
    variant: {
      primary: "btn-primary",
      secondary: "btn-outline",
      ghost: "btn-ghost",
      destructive: "btn-error",
    },
    size: {
      sm: "btn-sm",
      md: "",
      icon: "btn-square btn-sm",
    },
  },
  defaultVariants: { variant: "primary", size: "md" },
});

export interface ButtonProps
  extends ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, ...props }, ref) => (
    <button ref={ref} className={cn(buttonVariants({ variant, size }), className)} {...props} />
  ),
);
Button.displayName = "Button";

// A link that must look like a button no longer needs `asChild` + `Slot`:
// <a href="/messages" className={buttonVariants({ variant: "secondary", size: "sm" })}>View</a>
```

### 6.4 One `vaul` drawer sketch — the quick-vs-more distinction, encoded

Locks D14 as code, not just prose — direction, width, and dimming are baked into
two named exports so a call site cannot produce a drawer that's ambiguously
in-between.

**Flagged for whoever builds Bucket C (Phase 1), not fixed here — out of
Phase 0's own scope, but the mechanism is now known and worth checking
before this sketch is trusted verbatim.** The `useMediaQuery` call below has
the identical shape that broke `SideNav`'s SSR pass during Phase 0 (D12's
own updated entry has the full finding: `@uidotdev/usehooks`' `useMediaQuery`
hard-`throw`s in `getServerSnapshot`). A drawer's content usually never
renders during SSR — it's normally gated behind an `open` state that starts
`false` — but D14's own "more details" drawer is explicitly deep-linkable
(`?panel=<recordId>`, so it "survives refresh"), which means a page *can*
load with `open` already `true` on the very first server render. Whoever
implements this sketch for real should either confirm that path never
actually reaches this component during SSR, or apply the same fix `side-nav.tsx`
now uses (CSS-driven, not a JS breakpoint read) rather than assuming the
sketch's own `useMediaQuery` call is safe as written.

```tsx
// frontends/packages/ui/src/components/primitives/drawer.tsx
"use client";

import { Drawer as DrawerPrimitive } from "vaul";
import { cn } from "../../lib/cn";

export const Drawer = DrawerPrimitive.Root;
export const DrawerTrigger = DrawerPrimitive.Trigger;
export const DrawerClose = DrawerPrimitive.Close;

/** Quick details (§3): narrow, background stays legible, no route ownership.
 * Caller owns open/onOpenChange; caller does NOT own direction or width. */
export function QuickDetailContent({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  // Bottom sheet on a phone, right-hand panel from `md` up — expressed as
  // responsive classes, never a `useMediaQuery` read (D12). Both variants
  // are in the markup and CSS picks one, so this server-renders correctly
  // and there is no client-only boundary to pay for.
  return (
    <DrawerPrimitive.Portal>
      {/* deliberately no <DrawerPrimitive.Overlay> — background stays legible */}
      <DrawerPrimitive.Content
        className={cn(
          "fixed z-50 flex flex-col bg-surface-2 shadow-[var(--shadow-dialog)]",
          // phone: bottom sheet
          "inset-x-0 bottom-0 max-h-[70vh] rounded-t-box border-edge border-t",
          // md+: right-hand panel — reset the sheet-only properties explicitly
          "md:inset-x-auto md:inset-y-0 md:right-0 md:h-full md:max-h-none md:w-full md:max-w-[440px]",
          "md:rounded-t-none md:border-t-0 md:border-edge md:border-l",
          className,
        )}
      >
        {children}
      </DrawerPrimitive.Content>
    </DrawerPrimitive.Portal>
  );
}

/** More details (§3): wide, dims the background, expected to carry a
 * shallow route (`?panel=<id>`) at the call site — this component doesn't
 * own routing, only the visual weight that signals "this is a
 * destination." */
export function MoreDetailContent({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  const isMobile = useMediaQuery("(max-width: 767px)");
  return (
    <DrawerPrimitive.Portal>
      <DrawerPrimitive.Overlay className="fixed inset-0 z-50 bg-black/60" />
      <DrawerPrimitive.Content
        className={cn(
          isMobile
            ? "fixed inset-x-0 bottom-0 z-50 max-h-[92vh] rounded-t-box border-edge border-t"
            : "fixed inset-y-0 right-0 z-50 h-full w-full max-w-[680px] border-edge border-l",
          "flex flex-col bg-surface-2 shadow-[var(--shadow-dialog)]",
          className,
        )}
      >
        {children}
      </DrawerPrimitive.Content>
    </DrawerPrimitive.Portal>
  );
}
```

---

## 7. Build order

Sequenced so agents can work in parallel without editing the same file, and so
nobody builds a screen against a shell that hasn't landed yet.

**Phase 0 — tokens and shell (one agent, blocking, must land and typecheck before
Phase 1 starts):**
- `frontends/packages/ui/src/styles/theme.css` — delete the light theme (D9), rewrite the
  radius scale (D8).
- `frontends/packages/ui/src/components/primitives/theme-toggle.tsx` — delete.
- `frontends/packages/ui/package.json` — **add** `@headlessui/react`, `class-variance-authority`
  (`cva`), `@uidotdev/usehooks` (D3–D6, D11, D12). **Correction, found landing
  Phase 0: do not remove the `@radix-ui/*` packages here.** The line below this
  one originally said "swap Radix packages for `@headlessui/react`" — that's
  wrong for this phase specifically: nine primitives still `import` from
  `@radix-ui/*` and aren't ported until Phase 1 (Bucket A), so removing the
  dependencies now breaks `pnpm --filter @vsms/ui typecheck` for Phase 0 itself,
  which is exactly the gate this phase has to pass before Phase 1 can start.
  Every `@radix-ui/*` entry stays in `package.json` through Phase 0; Phase 1's
  Bucket A removes each one as it ports the primitive that uses it (D3–D6).
- New: `frontends/apps/admin/app/console-shell.tsx`, `frontends/apps/admin/app/nav-groups.ts` (§4 as data),
  `frontends/packages/ui/src/components/primitives/side-nav.tsx` (§6.2).
- `frontends/apps/admin/app/layout.tsx` — wrap in `ConsoleShell`.

**Phase 1 — primitives, three independent buckets (parallel, after Phase 0):**
- **Bucket A — Headless UI ports** (owns every file in the "Port"/"Rebuild" rows
  of §5 whose current basis is Radix): `dialog.tsx`, `dropdown-menu.tsx`,
  `label.tsx`, `popover.tsx`, `select.tsx`, `tabs.tsx` (+ new `ValueTabs`
  adapter, D18), `tooltip.tsx` → DaisyUI (D5), `separator.tsx` → plain div (D6).
- **Bucket B — CVA/DaisyUI refresh, no Radix involved** (owns `button.tsx` per
  §6.3, `badge.tsx`, `card.tsx`, `input.tsx`, `textarea.tsx`, `table.tsx` per
  D16, `skeleton.tsx`, `toast.tsx`): zero overlap with Bucket A's files.
- **Bucket C — drawer semantics** (owns `drawer.tsx` per §6.4 only): the
  narrowest bucket, one file, can land independently of A and B.

These three buckets touch disjoint files inside `frontends/packages/ui/src/components/
primitives/`, so they can run as three parallel agents with no merge conflicts;
`index.ts` re-exports are additive per file and merge cleanly.

**Phase 2 — screens, grouped by §4's IA (parallel, after Phase 0 + Phase 1 all
land and `frontends/packages/ui` typechecks clean):**
- **Agent "Messaging"** owns `frontends/apps/admin/app/page.tsx` (composer), `frontends/apps/admin/app/
  messages/**`, `frontends/apps/admin/app/simulator/**`.
- **Agent "Delivery"** owns `frontends/apps/admin/app/providers/**`, `frontends/apps/admin/app/routes/**`,
  `frontends/apps/admin/app/sender-ids/**`, `frontends/apps/admin/app/webhooks/**` — this is also the agent
  that builds the first real Quick/More-detail drawer pairs (§3), since every
  screen in this group needs one.
- **Agent "Operations"** owns `frontends/apps/admin/app/jobs/**`, `frontends/apps/admin/app/workers/**`,
  `frontends/apps/admin/app/opt-outs/**`.
- **Agent "Admin"** owns `frontends/apps/admin/app/apps/**`, `frontends/apps/admin/app/users/**`, `frontends/apps/admin/app/
  audit-log/**`, `frontends/apps/admin/app/settings/**`.
- **Agent "Dashboard+Gallery"** owns `frontends/apps/admin/app/dashboard/**` and `frontends/apps/admin/app/
  gallery/page.tsx` last, specifically because the gallery page is the one
  file that imports and exercises *every* `@vsms/ui` export — it is the natural
  final visual-QA surface, not a screen to build early.

Each screen agent only ever imports from `@vsms/ui`/`@vsms/gateway`/`@vsms/hooks`
— never from another screen file — so these five run with no collision risk
once Phases 0–1 are stable. `console-nav.tsx`'s per-screen header block (the
`LINKS` array duplicated ad hoc across files, per its own module doc) is deleted
entirely in Phase 0/2 in favor of `ConsoleShell` — no screen should still
hand-roll a `<header>` nav after Phase 2.

**Phase 3 — gate.** `pnpm --filter admin --filter @vsms/ui typecheck`, `pnpm
--filter admin build`, Biome check, and a manual pass through `/gallery` plus one
screen per IA group against this document's §1 reference lock (does the sidebar
match the LottieFiles structure? does the rounding match D8's register? does
every drawer resolve to exactly one of the two weights in §3?).

---

## 8. Risks

**What must survive completely untouched — this is not chrome, it is behavior a
redesign can silently regress if a screen gets rewritten instead of re-skinned:**

- **`messages-screen.tsx`'s live-reconciliation mechanism** — the self-scheduling
  long-poll loop (deliberately *not* `useQuery({ refetchInterval })`, because that
  combination was found live to stall after 1–2 calls), the `applyEvent` merge
  rules (in-place update never moves a row; scroll-position-gated buffering vs.
  direct insert; the sticky "N new" pill), and the cross-app visibility banner
  text. A screen agent re-skinning this file must preserve every `useRef`/`useEffect`
  in it verbatim and change only JSX/class names. Re-implementing "the same
  behavior" from scratch is exactly how the original `refetchInterval` bug would
  reappear.
- **`frontends/packages/gateway/src/request-credential.ts`'s `AsyncLocalStorage`-based
  per-request human-token forwarding**, and the two documented, deliberate
  exceptions to it (`client.ts`'s `sendMessage`/`previewMessage`, `messages.ts`'s
  `listMessagesForStream`) — nothing in this redesign touches `frontends/packages/gateway`
  at all, but a screen agent adding a *new* data-fetching call must resolve the
  token the same implicit way every existing call does, not import
  `getMachineAccessToken` by hand "for simplicity."
- **Layer-2 permission gates** (`require_permission` server-side, and each
  screen's own handling of a `403` — e.g. `providers-screen.tsx`'s real, proven
  `missing required permission "provider:update"` failure surfaced on screen, not
  swallowed). A rebuilt edit drawer must keep surfacing that exact failure mode,
  not silently retry or hide it.
- **`status-tokens.ts`'s eleven-state (message) / six-state (job) / five-state
  (attempt) semantics** — family, hue, glyph, and attention (quiet/loud) per
  state are a correctness artifact tied to real state-machine behavior
  (`uncertain` is not `failed`; a job's `failed` is retryable, its own `failed`
  is not styled like a message's terminal `failed`). §5 already scopes every
  status component to "class pass only" for exactly this reason — no build agent
  may change a hue, glyph, or family assignment as part of a visual refresh.
- **`nuqs` URL-state filters** on the Messages screen and any other filterable
  table — these are shareable/bookmarkable URLs today; a rebuild must not
  silently move filter state into component-local `useState`.
- **The OIDC login flow's PKCE/state/nonce cookie handling** (`frontends/apps/admin/middleware.ts`,
  `frontends/apps/admin/lib/oidc.ts`, `frontends/apps/admin/app/api/auth/**`) — entirely outside `frontends/packages/ui`
  and the route screens this document scopes, and must stay that way; the
  `/login` page itself (§4, not in the nav) gets only a visual pass, never a
  logic change.

**What could plausibly break if not handled deliberately:**

- **The D8 radius change is a deliberate, intended visual break from what
  shipped before, not a risk to be managed.** It replaces the prior 2px
  "restrained, data-forward" register outright, on this project's own
  standing hard-cutover convention. Phase 3's manual QA pass still checks it
  against the reference lock (§1.1/§1.2/§1.5) — the bar is "does this match
  what LottieFiles/Column/Polar actually show", not "does it look rounded"
  in the abstract — but the check is about fidelity to the reference, not
  about how far the number moved from 2px.
- **D10's `locale` removal touches four components' public API**
  (`StatusPill`, `StateTimeline`, and transitively `JobStatusPill`/
  `AttemptStatusPill`) — this is a breaking change to `@vsms/ui`'s exports.
  Every call site across all five Phase-2 screen groups must be grepped and
  updated in the same change that removes the prop, not left to fail silently
  Deleting `labelFr`/`tooltipFr` from the tables in the same change is what
  makes this safe rather than merely tidy: with the fields gone, a call site
  still passing `locale` or reading a French label is a **compile error**,
  where leaving them would let a missed call site keep compiling against data
  no longer reachable through any UI path. Do the deletion and the prop
  removal together, in one commit, and let `tsc` enumerate the call sites
  rather than trusting a grep.
- **D11's CVA migration must produce byte-identical class output** for the
  existing four components it touches (`button.tsx`, `badge.tsx`, and by
  extension anything composing them) — verified by diffing rendered class
  strings for every existing variant/size combination before merging Phase 1,
  not assumed from reading the `cva()` config.
- **D3's Headless UI `Select`→`Listbox` swap is the largest single API-shape
  change** in the whole primitives migration (§5, D17) — Radix `Select`'s
  `value`/`onValueChange` shape is close enough to `Listbox`'s that a naive port
  may compile but subtly change keyboard behavior (type-ahead, `Escape` handling)
  under Headless UI's own implementation; this needs its own focused manual
  keyboard-nav check in Phase 3, on the Messages-screen state filter
  specifically (the highest-traffic `Select` usage in the product).
- **RESOLVED — tested, not just flagged; a z-index fix was not the answer.**
  This entry originally predicted a z-index layering problem: `vaul` (Bucket
  C) and Headless UI `Dialog` (Bucket A) both portal, and a `Dialog`
  confirmation opened from inside a `MoreDetailDrawer` would need its
  z-index above the drawer's own. That framing was wrong about the actual
  failure mode, found live while building the Routes/Webhooks/Sender IDs
  screens: the nested `Dialog` never gets far enough to have a visible
  z-index problem. `MoreDetailDrawer`/`QuickDetailDrawer` always mount a
  trapped, document-level `@radix-ui/react-focus-scope` `FocusScope`
  (`vaul` never forwards its own `modal` prop down to
  `@radix-ui/react-dialog`'s `Root`, so Radix's default of `true` — and
  therefore the trap — applies unconditionally, regardless of `dimmed`),
  and Headless UI's `Dialog` always portals to a *sibling*
  `#headlessui-portal-root`, outside that trap's container. The trap
  force-refocuses back into the drawer the instant the nested `Dialog`
  tries to move focus into itself, permanently stalling its enter
  transition before it becomes visible at all — a confirmation stuck at
  `opacity: 0` forever, not a z-index collision. Four primitive-level
  fixes were tried and all four reproduced the stuck state; re-verified
  independently on a later pass, live, against the unmodified primitives,
  with the identical result both times, and with two more directions
  (`Dialog`'s own `autoFocus` prop; a nested Radix `Dialog` instead of
  Headless UI's) checked and closed for good — see
  `frontends/apps/admin/app/gallery/components/nested-dialog-in-drawer-regression.tsx`
  for the full writeup, including why the Radix alternative is foreclosed
  by D3 even apart from its own bugs. **The fix is `@vsms/ui`'s
  `InlineConfirm`** (§1.7, §3) — render the confirmation inline inside the
  drawer's own DOM subtree instead of nesting a second portaled, trapped
  overlay. `routes-screen.tsx`, `webhooks-screen.tsx`, `sender-ids-screen.tsx`,
  `apps-screen.tsx`, and `users-screen.tsx` all use it today (consolidated
  onto one shared implementation in #290). No further Phase 3 testing is
  needed for this case; the remaining z-index question (Bucket A's own
  `Dialog` needing an explicit `z-50`-or-higher, since it doesn't inherit
  one from Radix's old convention the way `vaul` still shares) applies only
  to a `Dialog` opened with **no** enclosing drawer, which was never the
  problem case.
- **Mobile-first is a real constraint on every screen agent, not just the
  shell** — five of the eighteen routes (Messages, Jobs, Workers, Providers,
  Routes) are dense multi-column tables; §1.6/§1.4's research answers "how does
  a dense table degrade" only at the pattern level (horizontal scroll inside a
  bordered container, quick-detail drawer instead of more columns), not per
  column-set. Each screen agent in Phase 2 must decide its own table's mobile
  column priority (which 2–3 columns stay visible under 480px, which move into
  the quick-detail drawer) — this document deliberately does not prescribe that
  per-screen, since it depends on each table's own field semantics.
