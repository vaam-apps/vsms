// Route-local (R6): the gallery's own outer wrapper + masthead — moved
// verbatim out of `page.tsx` (was the page's own markup, an R6 violation
// once the rest of this screen split into `components/**`).
//
// D5: DaisyUI's `.tooltip`/`data-tip` needs no provider — no wrapping
// component here any more (Headless UI has no Tooltip of its own either).
//
// Console-redesign Phase 2: this page used to render its own
// <main max-w-5xl px-6 py-10>, which is now nested inside `ConsoleShell`'s
// own <main> (Phase 0) — invalid HTML and doubled padding, the identical
// shape `dashboard-screen.tsx`'s own fix note describes. Replaced with a
// plain wrapper; the narrower max-w-5xl reading width is kept (a screen
// may choose its own content width inside the shared shell — the gallery
// reads better narrower than the 1400px shell default, since it's mostly
// prose plus small demo blocks, not a dense table). `<Toaster />` is
// dropped entirely — see `page.tsx`'s own header comment for why.
//
// Not built on `ScreenStack`/`ScreenHeader` (`@vsms/ui`) — this page's own
// masthead carries an eyebrow line and a max-w-5xl reading width, both
// genuinely different from the standard console screen chrome those two
// components give every other route, and this is the one screen in the
// console that isn't a real product surface (it's the visual-QA page for
// `@vsms/ui` itself). Route-local per R6's own "encodes this screen's own
// shape" test, not a case of reinventing the standard pair.

import type { ReactNode } from "react";

export function GalleryLayout({ children }: { children: ReactNode }) {
  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-10">
      <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
        <div>
          <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
            @vsms/ui — T6
          </p>
          <h1 className="mt-1 font-medium text-foreground text-title">Component gallery</h1>
          <p className="mt-1 max-w-2xl text-body text-muted-foreground">
            An honest rendering of the status system and every primitive — not a fake dashboard.
            This page's only job is to prove the design tokens, daisyUI theming, and behaviour
            actually work. Dark-only (D9) — there is no second theme to switch to.
          </p>
        </div>
      </header>

      {children}
    </div>
  );
}
