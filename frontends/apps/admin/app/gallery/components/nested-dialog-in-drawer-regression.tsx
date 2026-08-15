"use client";

// Route-local (R6): moved verbatim out of `page.tsx` — this comment and the
// component below it are the single most valuable thing in this route, per
// this repo's own "record the finding, don't delete it" convention. Do not
// summarise or trim it on a future edit; append to it instead.
//
// console-redesign.md §3/D14: the two baked-direction, baked-dim drawer
// variants Phase 2's "Delivery" agent builds every Provider/Route/
// Sender ID/Webhook quick-vs-more pair on top of. This is the QA surface
// for both — resize the browser pane to check the phone/desktop split
// (base = bottom sheet, `md`+ = right panel) and confirm quick details
// never dims while more details does.
//
// **RESOLVED — was a known, 100%-reproducing bug; kept here as a
// regression demo, per this repo's own "record the finding, don't delete
// it" convention.** The investigation and everything that didn't work are
// preserved below verbatim; only the verdict and the demo itself changed.
//
// The bug, as found: nesting a centered `Dialog` (Headless UI) inside an
// open `MoreDetailDrawer` (vaul, `modal=true`) left the confirmation
// permanently invisible — `opacity: 0`, its enter transition never
// settled, and keyboard focus snapped straight back to whatever triggered
// it. Not the drawer dismissing itself — both `open` states stayed `true`
// forever — the confirmation became a stuck, non-interactive ghost behind
// an opaque drawer that never actually went anywhere.
//
// Root cause, confirmed by live DOM/focus instrumentation, not guessed:
// `MoreDetailDrawer`'s `modal={true}` routes vaul's `Content` through
// `@radix-ui/react-dialog`'s `DialogContentModal`, which mounts a
// `@radix-ui/react-focus-scope` `FocusScope` with `trapped: true` — a
// **document-level** `focusin` listener that force-refocuses back into
// the drawer's own container the instant focus lands anywhere outside
// it. Headless UI's `Dialog` always portals to its own
// `#headlessui-portal-root`, a *sibling* top-level `<body>` child (its
// outer `Portal` is wrapped in `ForcePortalRoot force={true}`,
// specifically blocking `Portal.Group` redirection — there is no
// supported way to make it portal *into* vaul's own container instead).
// So the moment the confirmation tried to move focus into itself, Radix
// yanked it straight back, permanently stalling Headless UI's own CSS
// enter transition mid-flight.
//
// Four independent primitive-level fixes were tried and all four still
// reproduced the stuck state on live trials: toggling vaul's `modal` off
// while the nested `Dialog` is open (`useEffect`-timed and, separately,
// `useLayoutEffect`-timed signals — the latter chosen specifically
// because Headless UI's own initial-focus move is deferred via
// `queueMicrotask`, which should have made a layout effect win the
// race); `initialFocus` pointed at a known-stable ref; and the two
// combined. The `useEffect`/`useLayoutEffect` attempts also hit a real,
// independent bug in `vaul@1.1.2`'s own `Overlay` component — it calls
// `useCallback` *after* an `if (!modal) return null` early return in its
// render body, so changing `modal` on an already-mounted `Overlay`
// crashed the whole app with "Rendered fewer hooks than expected."
//
// No reliable fix exists confined to `drawer.tsx`/`dialog.tsx` given
// `vaul@1.1.2` + `@radix-ui/react-dialog@1.1.23` +
// `@headlessui/react@2.2.10` as pinned — `console-redesign.md`'s original
// §3/§1.7 "centered Dialog opened from inside MoreDetailDrawer" pattern
// was not safely buildable as written (both sections are now corrected to
// say so). The bug required the nested `Dialog` to open while a
// `MoreDetailDrawer` (`dimmed`, i.e. vaul `modal=true`) was genuinely
// still open behind it — checked against every `Dialog` trigger in all
// four merged Delivery screens, not assumed:
//   - Affected: `routes-screen.tsx`'s "Delete this route?" (footer Delete
//     inside its `MoreDetailDrawer`); `webhooks-screen.tsx`'s "Delete this
//     endpoint?" and "Rotate this endpoint's secret?" (both inside its
//     `MoreDetailDrawer`); `sender-ids-screen.tsx`'s "Register {value}
//     with a provider" (inside its first `MoreDetailDrawer`) and
//     "Resubmit this registration?" (inside its second, stacked
//     `MoreDetailDrawer`).
//   - ALSO affected, corrected after review: `webhooks-screen.tsx`'s
//     "Replay this delivery?". An earlier revision of this comment claimed
//     it was immune because it opens from a `QuickDetailDrawer`, whose
//     `dimmed={false}` was assumed to make vaul's `modal` false and leave
//     Radix's `FocusScope` un-`trapped`. That is wrong, and `drawer.tsx`'s
//     own module comment already said so: **vaul never forwards its
//     `modal` prop down to `@radix-ui/react-dialog`'s `Root`**, which
//     keeps its own default of `true` regardless — so the focus trap and
//     `aria-modal` are unconditional, and `dimmed` changes only the
//     overlay and background pointer-events. Re-verified three ways
//     (vaul@1.1.2 compiled source, a jsdom listener check, and a real
//     browser harness importing the unmodified primitives): the identical
//     stuck-invisible symptom reproduced inside a `QuickDetailDrawer`.
//     Six confirmations were broken, not five.
//     Not affected: every screen's own top-level "New X" create
//     dialog (`webhooks-screen.tsx`, `sender-ids-screen.tsx`) — triggered
//     from a toolbar button reachable only when no drawer is open.
//   - `providers-screen.tsx` uses no `Dialog` at all — no nested
//     confirmation existed there to be affected.
//
// A live trial of the theoretical alternative (a real `@radix-ui/react-
// dialog` `Dialog.Root` nested inside vaul's own Radix-based `Content`,
// confirmed to share the exact same `@radix-ui/react-dismissable-layer`/
// `@radix-ui/react-focus-scope` module instances as vaul itself, not a
// second copy) did **not** reproduce the stuck-invisible symptom — the
// nested Radix dialog rendered and opened correctly. But it was not a
// clean drop-in either: confirming inside it also closed the outer drawer
// in that trial, a different, apparently DismissableLayer-outside-click-
// related side effect, and Radix's own `useCallback`/`FocusScope` chatter
// (repeated `focusin` back to the trigger) still showed up even though
// the dialog itself stayed visible throughout. Not pursued further, for
// the same reason the fix below wasn't a primitive-level one: the actual
// fix is a *screen-level* pattern change, not a smarter nested overlay.
//
// **The fix, shipped:** `@vsms/ui`'s `InlineConfirm`
// (`components/primitives/inline-confirm.tsx`) renders the confirmation
// **inline, inside the drawer's own DOM subtree** — no portal, no second
// `FocusScope`, nothing for vaul's own trap to fight. The caller swaps the
// drawer's `children` (and drops its `footer` prop, since `InlineConfirm`
// supplies its own Cancel/Confirm row) instead of layering a second
// overlay on top. `routes-screen.tsx`, `webhooks-screen.tsx`, and
// `sender-ids-screen.tsx` all converted their six broken confirmations to
// this pattern in the same change that added this regression demo.
// `apps-screen.tsx`'s `ProvisionClientPanel` and `users-screen.tsx`'s own
// delete confirmations hit the identical bug independently while being
// built on top of this file, each shipping its own inline panel before
// this component existed to point at — #290 folded all three into this
// one shared `InlineConfirm`, so there is now exactly one implementation
// of "confirmation nested in an open drawer" in this codebase, not four.
//
// **Independent re-verification (a later pass, prompted by a bug report
// that described a *different* symptom — "the drawer self-dismisses
// within ~0.5s, no visible confirmation ever appears" — for the same
// `MoreDetailDrawer`+`Dialog` nesting).** Reproduced live against the
// unmodified primitives (a temporary, scratch-only gallery mount, driven
// via real DOM events and polled with `getComputedStyle`/
// `document.activeElement` at +0ms/+300ms/+1500ms after the nested
// `Dialog` opens — not a screenshot, not a guess): the self-dismiss
// symptom **did not reproduce, in either drawer variant**. What was
// observed, both times, matches this file's own verdict exactly —
// `data-vaul-drawer`'s `data-state` stays `"open"` throughout, the nested
// `DialogPanel` stays at `opacity: 0` forever, and `document.activeElement`
// settles on a `<div>` inside the drawer's own `FocusScope` boundary (the
// trap's fallback target), never the trigger, never the panel. Whatever
// produced the "self-dismisses" report was not this bug as it exists in
// this exact `vaul@1.1.2` + `@radix-ui/react-dialog@1.1.23` +
// `@headlessui/react@2.2.10` combination — treat any future report of an
// outer drawer *closing* (rather than a nested confirmation staying stuck
// and invisible) as a materially different bug and re-diagnose it fresh
// rather than assuming this writeup already covers it.
//
// Two more primitive-level directions were checked on this pass and both
// are closed, not merely untried:
//
// 1. **Headless UI's `Dialog` `autoFocus` prop cannot suppress the
//    initial-focus grab that trips Radix's trap**, read directly from
//    `@headlessui/react@2.2.10`'s compiled `focus-trap.js`. The grab is
//    gated by the `InitialFocus` feature bit, which `dialog.js` sets
//    whenever `!isTouchDevice()` — computed internally, not read from any
//    prop. `autoFocus={false}` only clears the separate `AutoFocus` bit,
//    which changes *which* focusable descendant gets chosen
//    (`Focus.AutoFocus` vs `Focus.First` strategy) but does not gate
//    *whether* `FocusTrap` moves focus on mount at all. The only prop that
//    disables the grab outright is `__demoMode` — a double-underscore,
//    undocumented prop that reads as Headless UI's own documentation-site
//    internal, not a supported public API; shipping product code against
//    it would trade one unreliable behavior for a dependency on
//    unspecified library internals, not a fix.
// 2. **The nested-Radix-dialog alternative this file already flagged as
//    "one trial only... unverified"** — even setting aside that its own
//    trial found a second bug (confirming also closed the outer drawer) —
//    is foreclosed for a reason that has nothing to do with focus traps:
//    it requires importing `@radix-ui/react-dialog` directly into
//    `@vsms/ui`, which is the exact dependency `console-redesign.md`'s own
//    decision ledger (D3) already replaced with Headless UI, deliberately,
//    for every primitive that has a Headless UI equivalent. Making this
//    one case an exception would mean carrying two competing modal/focus-
//    trap implementations in this package permanently, for one call site —
//    worse than the inline-panel pattern even if its own bug were fixed.
//
// Verdict unchanged, now on firmer footing: no fix confined to
// `drawer.tsx`/`dialog.tsx` is both reliable and consistent with this
// package's own architecture. `InlineConfirm` is not a stopgap standing in
// for a primitive fix that might still land later — it is the fix.

import { Button, InlineConfirm, MoreDetailDrawer } from "@vsms/ui";
import { useState } from "react";
import { Section } from "./section";

export function NestedDialogInDrawerRegression() {
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [confirmArmed, setConfirmArmed] = useState(false);
  const [confirmed, setConfirmed] = useState(false);

  return (
    <Section
      title="Regression: inline confirmation inside an open MoreDetailDrawer"
      description="Open the drawer, then click Delete — the confirmation renders inline, is visible, and is interactive. See this file's own header comment for the full root-cause writeup, every fix attempted, and the fix that shipped."
    >
      <Button
        variant="secondary"
        onClick={() => {
          setDrawerOpen(true);
          setConfirmArmed(false);
          setConfirmed(false);
        }}
      >
        Open more details
      </Button>

      <MoreDetailDrawer
        open={drawerOpen}
        onOpenChange={(open) => {
          setDrawerOpen(open);
          if (!open) setConfirmArmed(false);
        }}
        title="Webhook endpoint"
        description="A stand-in for webhooks-screen.tsx's own delete-endpoint drawer."
        footer={
          confirmArmed ? undefined : (
            <Button variant="destructive" size="sm" onClick={() => setConfirmArmed(true)}>
              Delete
            </Button>
          )
        }
      >
        {confirmArmed ? (
          <InlineConfirm
            title="Delete this endpoint?"
            description="This action cannot be undone."
            confirmLabel="Delete"
            onConfirm={() => {
              setConfirmed(true);
              setConfirmArmed(false);
            }}
            onCancel={() => setConfirmArmed(false)}
          />
        ) : (
          <p className="text-body text-muted-foreground">
            Clicking Delete swaps this body to an inline `InlineConfirm` — no nested `Dialog`, no
            second focus trap. {confirmed && "Confirmed on the last run."}
          </p>
        )}
      </MoreDetailDrawer>
    </Section>
  );
}
