// Dumb component (R6): the console's own drawer/top-bar/sidebar structure.
// Markup moved verbatim out of `console-shell.tsx` — that file now only
// decides *whether* to render this chrome (the `/login` bare-render check)
// and builds the `accountSlot`/`NAV_*` data it hands down; every
// `className` and every piece of layout markup lives here, `SideNav`
// included.
//
// `CONSOLE_NAV_TOGGLE_ID` moved from a screen-level const into this
// component's own internal wiring — it only exists to link the drawer
// checkbox to its two toggle labels (the mobile hamburger, the overlay),
// which are both rendered by this file alone now, so no caller ever needs
// to know the id exists.
//
// A prior revision of `console-shell.tsx` wrapped `SideNav` in
// `next/dynamic({ ssr: false })`, working around a real bug (`@vsms/ui`'s
// `SideNav` used to call `@uidotdev/usehooks`' `useMediaQuery` (D12), and
// that hook's own `getServerSnapshot` is a hard
// `throw new Error("useMediaQuery is a client-only hook")` — every full
// page load under this shell 500'd) by never server-rendering the nav at
// all — paid for with an empty sidebar slab on every first paint,
// hydrating in afterward. Reworked instead: `side-nav.tsx` no longer reads
// the viewport in JS anywhere — the three breakpoint bands are plain
// Tailwind `lg:`/`xl:` responsive classes, so `SideNav` is an ordinary
// import again and renders in the initial server-rendered HTML like
// everything else in this shell. See `side-nav.tsx`'s own module doc for
// the full mechanism, and its `NavLink`/`GroupSection` doc comments for
// why a JS breakpoint read isn't needed here in the first place.
//
// Postscript: `@uidotdev/usehooks` is no longer a dependency of this repo
// at all. Once the rework above removed the only call site, it was a
// declared package with zero live imports — and its last real publish was
// 2023-10-23, predating React 19, which this console runs on. D12 in
// `docs/design/console-redesign.md` carries the full reasoning. The
// package name survives in this comment only as the record of a bug worth
// not repeating: a hook that throws in `getServerSnapshot` fails every
// server-rendered page while `pnpm build` stays green, because every route
// here is dynamic and build-time generation never executes them.

import { type NavGroup, type NavItem, SideNav } from "@vsms/ui";
import { Menu } from "lucide-react";
import type { ReactNode } from "react";

const CONSOLE_NAV_TOGGLE_ID = "console-nav";

export interface ConsoleChromeProps {
  topItem: NavItem;
  groups: NavGroup[];
  footerItems: NavItem[];
  currentPath: string;
  accountSlot: ReactNode;
  children: ReactNode;
}

export function ConsoleChrome({
  topItem,
  groups,
  footerItems,
  currentPath,
  accountSlot,
  children,
}: ConsoleChromeProps) {
  return (
    <div className="drawer lg:drawer-open">
      <input id={CONSOLE_NAV_TOGGLE_ID} type="checkbox" className="drawer-toggle" />

      <div className="drawer-content flex min-h-dvh flex-col">
        {/* Slim sticky top bar — visible at every breakpoint; carries the
            hamburger below `lg`, wordmark always. §6.2's own sketch also
            names a search field and account control living here; neither
            exists yet (no search backend, and the account control already
            lives in the sidebar's own footer per `accountSlot` above) —
            left for a later screen agent to add, not stubbed. */}
        <header className="sticky top-0 z-30 flex h-14 shrink-0 items-center gap-3 border-edge border-b bg-base-200 px-4">
          <label
            htmlFor={CONSOLE_NAV_TOGGLE_ID}
            className="btn btn-square btn-ghost btn-sm lg:hidden"
            aria-label="Open navigation"
          >
            <Menu size={18} aria-hidden="true" />
          </label>
          <span className="font-mono text-caption text-subtle-foreground">vsms</span>
        </header>

        <main className="mx-auto w-full max-w-[1400px] flex-1 px-4 py-6 lg:px-8 lg:py-10">
          {children}
        </main>
      </div>

      <div className="drawer-side z-40">
        <label
          htmlFor={CONSOLE_NAV_TOGGLE_ID}
          aria-label="Close navigation"
          className="drawer-overlay"
        />
        <SideNav
          topItem={topItem}
          groups={groups}
          footerItems={footerItems}
          currentPath={currentPath}
          accountSlot={accountSlot}
          className="min-h-dvh w-[260px] border-edge border-r lg:w-[64px] xl:w-[260px]"
        />
      </div>
    </div>
  );
}
