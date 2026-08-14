"use client";

import { X } from "lucide-react";
import { forwardRef, type ReactNode } from "react";
import { Drawer as DrawerPrimitive } from "vaul";
import { cn } from "../../lib/cn";

// vaul is standalone. Direction defaults to "right" — a side drawer reads
// as the console's Sheet-equivalent (design doc T15: "extend, don't fork");
// pass `direction="bottom"` for the mobile/narrow-viewport case.
//
// This generic composition (`Drawer`/`DrawerTrigger`/`DrawerClose`/
// `DrawerContent`) is unchanged by console-redesign.md §6.4/D14 — it is
// still the right tool for a one-off drawer with no quick-vs-more
// distinction to encode (see the gallery's own "Open drawer" example).
// `QuickDetailDrawer`/`MoreDetailDrawer` below are a *second*, self-
// contained API for the specific §3 distinction; they do not replace this
// one.
export const Drawer = DrawerPrimitive.Root;
export const DrawerTrigger = DrawerPrimitive.Trigger;
export const DrawerClose = DrawerPrimitive.Close;
export const DrawerPortal = DrawerPrimitive.Portal;

export const DrawerOverlay = forwardRef<
  React.ElementRef<typeof DrawerPrimitive.Overlay>,
  React.ComponentPropsWithoutRef<typeof DrawerPrimitive.Overlay>
>(({ className, ...props }, ref) => (
  <DrawerPrimitive.Overlay
    ref={ref}
    className={cn("fixed inset-0 z-50 bg-black/50", className)}
    {...props}
  />
));
DrawerOverlay.displayName = "DrawerOverlay";

export const DrawerContent = forwardRef<
  React.ElementRef<typeof DrawerPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DrawerPrimitive.Content>
>(({ className, children, ...props }, ref) => (
  <DrawerPortal>
    <DrawerOverlay />
    <DrawerPrimitive.Content
      ref={ref}
      className={cn(
        "fixed inset-y-0 right-0 z-50 flex h-full w-full max-w-[560px] flex-col",
        "border-edge border-l bg-surface-2 shadow-[var(--shadow-dialog)]",
        className,
      )}
      {...props}
    >
      {children}
    </DrawerPrimitive.Content>
  </DrawerPortal>
));
DrawerContent.displayName = "DrawerContent";

/** Not previously exported — `vaul`'s `Content` renders Radix Dialog's
 * `Content` underneath (`vaul/dist/index.js` imports `@radix-ui/react-
 * dialog` directly and forwards its `Content`/`Title`/`Description`), so
 * Radix's own "DialogContent requires a DialogTitle" dev warning applies
 * here exactly as it does to `primitives/dialog.tsx`. `QuickDetailDrawer`/
 * `MoreDetailDrawer` below always render one; exported separately too for
 * any future one-off use of the generic `DrawerContent` above. */
export const DrawerTitle = forwardRef<
  React.ElementRef<typeof DrawerPrimitive.Title>,
  React.ComponentPropsWithoutRef<typeof DrawerPrimitive.Title>
>(({ className, ...props }, ref) => (
  <DrawerPrimitive.Title
    ref={ref}
    className={cn("font-medium text-foreground text-title-sm", className)}
    {...props}
  />
));
DrawerTitle.displayName = "DrawerTitle";

/** See `DrawerTitle` — same "not previously exported, Radix warns without
 * one" reasoning, for `aria-describedby` instead of the accessible name. */
export const DrawerDescription = forwardRef<
  React.ElementRef<typeof DrawerPrimitive.Description>,
  React.ComponentPropsWithoutRef<typeof DrawerPrimitive.Description>
>(({ className, ...props }, ref) => (
  <DrawerPrimitive.Description
    ref={ref}
    className={cn("text-muted-foreground text-prose", className)}
    {...props}
  />
));
DrawerDescription.displayName = "DrawerDescription";

// ---------------------------------------------------------------------
// QuickDetailDrawer / MoreDetailDrawer (console-redesign.md §3, D14, §5)
// ---------------------------------------------------------------------
//
// The design doc's own §6.4 sketch of this file was explicitly flagged as
// unverified ("the mechanism is now known and worth checking before this
// sketch is trusted verbatim") for two independent reasons, both checked
// directly against `vaul@1.1.2`'s compiled source
// (`node_modules/vaul/dist/index.js`) before writing this, not assumed:
//
// 1. **The `useMediaQuery` call the sketch used is gone (D12).** No JS
//    viewport read of any kind happens in this file — every breakpoint
//    switch below is a plain Tailwind `md:` variant, so both variants
//    render identically on the server and on the client's first paint.
//
// 2. **A single `vaul` `Drawer.Root` cannot itself be "bottom on phone,
//    right on desktop" — its `direction` prop is not a CSS concern.** It
//    drives real per-instance state: which axis (`translate3d(0,y,0)` vs
//    `(x,0,0)`) the built-in open/close keyframes and the live drag-follow
//    math use (`isVertical(direction)`, `getTranslate(el, direction)`,
//    throughout `dist/index.js`). The sketch never set `direction` at all
//    (it left every caller to supply it on a separately-rendered `<Drawer>`
//    Root, which — combined with `direction`'s own default of `"bottom"` —
//    means the sketch's "more details" `Content` would in practice always
//    have inherited `"bottom"` regardless of viewport anyway). Rather than
//    read the viewport to pick a direction (banned, and also the same
//    hydration-mismatch shape D12 already fixed once — a deep-linked
//    `?panel=<id>` drawer can be `open` on the very first server render,
//    per D14, so the first paint has to be right, not just eventually
//    correct after an effect), both drawers below **fix `direction` to
//    `"bottom"` unconditionally** and let plain `md:` classes handle
//    *position* — matching this file's own established, precedented
//    pattern for a responsive split that can't be one reactive value
//    (`side-nav.tsx`'s `GroupSection`: "two parallel trees... not one tree
//    whose content changes based on a JS-read viewport width").
//
//    The one real, accepted cost of fixing `direction`: `vaul`'s own
//    open/close animation (`slideFromBottom`/`slideToBottom`, a vertical
//    `translate3d`) plays at every breakpoint, including the `md:` right-
//    panel position — so the panel's *entrance motion* is vertical even
//    though its *resting position* is a right-hand edge. This was
//    deliberately not "fixed" by layering a second, horizontal CSS
//    `transition` on top keyed off `data-[state=]`: `vaul`'s animation is
//    a real `@keyframes` `animation`, which (unlike a plain `transition`)
//    correctly plays on a freshly-inserted node — Radix's `Content` isn't
//    in the DOM at all until `open`, so a competing `transition`-based
//    replacement would very likely not animate on *open* at all (no prior
//    DOM node to transition *from*) while still fighting the real
//    animation for the `transform` property. Keeping `vaul`'s own,
//    already-correct mechanism and accepting a vertical entrance for the
//    desktop panel is the smaller, more honest compromise, and it costs no
//    extra CSS (constraint 7). If this reads as wrong once seen live,
//    revisit by disabling it explicitly (`data-vaul-animate="false"` is
//    force-overridable via a prop — `Content`'s `...rest` spreads *after*
//    vaul's own default in `dist/index.js`) and shipping a real
//    `@keyframes`-based replacement, not a `transition`.
//
//    Fixing `direction` also fixes the *drag* gesture to the vertical
//    axis everywhere. Left alone, that would make a click-drag inside the
//    desktop panel (e.g. selecting text) register as a vertical swipe
//    attempt. Closed with `handleOnly` on the `Root` (drag can only start
//    from a `Drawer.Handle`) plus rendering that handle only below `md:` —
//    so desktop has no drag surface at all (pointer/click/Escape/overlay
//    close it instead), and phone keeps a real, correctly-axised drag
//    handle, which is also what §3's own mobile paragraph asks for ("...
//    with a drag handle").
//
// Both variants are otherwise identical in mechanism and differ only in
// the two things §3/D14 make load-bearing: `dimmed` (renders an
// `Overlay` and sets `vaul`'s own `modal` prop — verified live in
// `dist/index.js` that `modal` is what actually gates both the
// background `pointer-events`/scroll-lock behaviour *and*, independently,
// whether an `Overlay` should render at all; Radix's own focus trap and
// `aria-modal="true"` are unconditional either way — `vaul` never forwards
// its `modal` prop down to `@radix-ui/react-dialog`'s `Root`, which keeps
// its own default of `true` regardless. So "quick details" is visually and
// interactively lighter — no dim, background stays scrollable/clickable —
// but is exactly as keyboard-trapped and exactly as announced to a screen
// reader as "more details" is; that's a real, verified limitation of
// `vaul@1.1.2`, not a gap left open on purpose) and `contentClassName`
// (width/inset/radius/border, per D14's own two size ranges).
interface DetailDrawerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Rendered as the drawer's accessible name (`Drawer.Title`) — required,
   * not optional, because both variants always render one and Radix warns
   * loudly in dev without it. */
  title: ReactNode;
  /** Rendered as the drawer's accessible description (`Drawer.Description`).
   * Optional for the *caller* — when omitted, a screen-reader-only
   * fallback is still rendered so `aria-describedby` is always wired,
   * matching `primitives/dialog.tsx`'s own precedent of never shipping a
   * `Content` with no description at all. */
  description?: ReactNode;
  /** The scrollable body — a summary of fields, an edit form, whatever the
   * calling screen owns. This file has no opinion on it. */
  children: ReactNode;
  /** Optional sticky action row (§1.5's Polar reference: "a fixed bottom
   * action row... stays visible without the form needing to scroll"). */
  footer?: ReactNode;
  className?: string;
}

function DetailDrawerContent({
  open,
  onOpenChange,
  title,
  description,
  children,
  footer,
  className,
  dimmed,
  contentClassName,
}: DetailDrawerProps & { dimmed: boolean; contentClassName: string }) {
  return (
    <DrawerPrimitive.Root
      open={open}
      onOpenChange={onOpenChange}
      direction="bottom"
      modal={dimmed}
      dismissible
      handleOnly
      // vaul defaults this to `false` (`onOpenAutoFocus` is pre-vented
      // unless the caller opts in) — verified in `dist/index.js`'s `Root`
      // default parameters. Without it, opening either drawer would leave
      // keyboard focus sitting on whatever triggered it instead of moving
      // into the panel, which is a real, checkable regression from plain
      // Radix `Dialog` behaviour (its own default *does* auto-focus
      // `Content`) — set explicitly so focus genuinely lands inside the
      // drawer the moment it opens, not just gets trapped there once the
      // user starts tabbing.
      autoFocus
    >
      <DrawerPrimitive.Portal>
        {dimmed && <DrawerPrimitive.Overlay className="fixed inset-0 z-50 bg-black/50" />}
        <DrawerPrimitive.Content
          className={cn(
            "fixed z-50 flex flex-col bg-surface-2 shadow-[var(--shadow-dialog)] outline-none",
            // Phone: bottom sheet.
            "inset-x-0 bottom-0 rounded-t-box border-edge border-t",
            // `md`+: right-hand panel — every sheet-only property above is
            // reset explicitly (inset, rounding, border side, max-height),
            // not just overridden by a wider max-width, per this file's
            // own D12-correction note above.
            "md:inset-x-auto md:inset-y-0 md:right-0 md:h-full md:max-h-none",
            "md:w-full md:rounded-t-none md:rounded-l-box md:border-t-0 md:border-l",
            contentClassName,
            className,
          )}
        >
          {/* Drag surface (D14's mobile "with a drag handle"), phone only —
              `handleOnly` on Root means dragging is only possible starting
              from this element, and it's hidden at `md:`, so desktop has
              no drag surface at all (see this file's own header comment
              for why: `direction` is fixed to "bottom", so a desktop drag
              would otherwise compute against the wrong axis).
              **`!` (Tailwind v4 important) is load-bearing here, not
              decorative — found live, not assumed:** `vaul` injects
              `[data-vaul-handle]{display:block; ...}` into a `<style>` tag
              it appends to `<head>` itself (`dist/index.js`'s own
              `__insertCSS`), at the same specificity as a plain `md:hidden`
              utility and *after* Tailwind's build-time stylesheet in
              cascade order — so a bare `md:hidden` lost the tie and the
              handle stayed visible at desktop width, confirmed by
              inspecting `getComputedStyle(...).display` in a real browser
              at 1280px before this fix, and confirmed gone after it. */}
          <DrawerPrimitive.Handle className="mt-2 shrink-0 bg-edge-strong md:hidden!" />

          <div className="flex items-start justify-between gap-3 border-edge border-b px-5 py-4">
            <div className="min-w-0">
              <DrawerTitle className="truncate">{title}</DrawerTitle>
              <DrawerDescription className={description == null ? "sr-only" : "mt-1"}>
                {description ?? "Details panel."}
              </DrawerDescription>
            </div>
            <DrawerPrimitive.Close
              aria-label="Close"
              className="shrink-0 text-subtle-foreground hover:text-foreground"
            >
              <X size={16} strokeWidth={1.5} />
            </DrawerPrimitive.Close>
          </div>

          <div className="flex-1 overflow-y-auto px-5 py-4">{children}</div>

          {footer != null && (
            <div className="flex shrink-0 items-center justify-end gap-2 border-edge border-t px-5 py-4">
              {footer}
            </div>
          )}
        </DrawerPrimitive.Content>
      </DrawerPrimitive.Portal>
    </DrawerPrimitive.Root>
  );
}

/**
 * **Quick details** (§3, D14) — a peek at one row's state without leaving
 * the list. Narrow (`420–480px` at `md`+), undimmed (background stays
 * legible and, per `vaul`'s own `modal={false}` behaviour verified above,
 * scrollable/clickable — matching the Mercury reference, §1.4), no route
 * ownership: the caller owns `open`/`onOpenChange` from local state (e.g.
 * "which row id is selected"), and losing it on refresh is expected and
 * fine (reopening is one click on the same row).
 *
 * Not a `Dialog`, and not for anything destructive — §1.7/§3 reserve that
 * for the centered `Dialog` primitive. This is a read-mostly summary with
 * 1–2 actions, e.g. a "View full details" link that upgrades to
 * `MoreDetailDrawer`.
 */
export function QuickDetailDrawer(props: DetailDrawerProps) {
  return (
    <DetailDrawerContent
      {...props}
      dimmed={false}
      contentClassName="max-h-[70vh] md:max-w-[440px]"
    />
  );
}

/**
 * **More details** (§3, D14) — the full record: every field, an edit
 * form, destructive actions, short nested history. Wide (`640–720px` at
 * `md`+), dimmed (modal-weight, matching the Polar reference, §1.5). The
 * *caller* is expected to own a shallow `?panel=<recordId>` route so this
 * survives refresh and is linkable (D14) — this component only owns the
 * visual/behavioural weight, never routing; pass `open` derived from that
 * query param and `onOpenChange` wired to update it.
 *
 * A destructive-confirmation step opened *from inside* this drawer (e.g.
 * webhook-secret rotation) stays a nested `Dialog` (§3's own footnote) —
 * that dialog needs a z-index at or above this drawer's `z-50`; setting
 * that is `primitives/dialog.tsx`'s job (§8 risk list), not this file's.
 */
export function MoreDetailDrawer(props: DetailDrawerProps) {
  return <DetailDrawerContent {...props} dimmed contentClassName="max-h-[92vh] md:max-w-[680px]" />;
}
