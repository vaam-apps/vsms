"use client";

// The console's app shell (docs/design/console-redesign.md §6.2, Phase 0).
// Replaces `console-nav.tsx`'s per-screen header pattern with one shared
// chrome mounted once from `admin/app/layout.tsx` — see that file's own
// module doc for what a per-screen re-skin must NOT change about data
// fetching (messages-screen.tsx's live-poll loop, AsyncLocalStorage token
// forwarding, Layer-2 403 surfacing, nuqs URL state, the OIDC session
// handling — none of that lives here or is touched by this file).
//
// `console-nav.tsx` itself is left in place for now, still used by the
// five screens that already reference it (apps/audit-log/opt-outs/
// settings/users) — deleting *those* screens' own hand-rolled headers is
// Phase 2's job (per §7's own build order: "no screen should still
// hand-roll a <header> nav after Phase 2"), not this shell-only PR's.

import { SideNav } from "@vsms/ui";
import { Menu } from "lucide-react";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { NAV_FOOTER, NAV_GROUPS, NAV_TOP } from "./nav-groups";

const CONSOLE_NAV_TOGGLE_ID = "console-nav";

// A prior revision of this file wrapped `SideNav` in
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

export function ConsoleShell({
  children,
  accountEmail,
}: {
  children: ReactNode;
  /** The signed-in human's email, read server-side in `layout.tsx` off the
   * `x-vsms-actor` header `admin/middleware.ts` already sets on every
   * authenticated request (the same header `packages/api/src/context.ts`
   * reads for the tRPC `actor` field) — display only, not an auth check;
   * `middleware.ts` has already decided whether this request is allowed
   * through by the time this component ever renders. */
  accountEmail?: string | null;
}) {
  const pathname = usePathname();

  // `/login` is a pre-auth gate with no shell (§4) — render it bare rather
  // than teaching this component anything about the auth flow itself.
  if (pathname === "/login") {
    return <>{children}</>;
  }

  const accountSlot = (
    <div className="flex flex-col gap-1 text-caption text-subtle-foreground">
      {accountEmail != null && accountEmail !== "" && (
        <p className="truncate">
          Signed in as <span className="text-muted-foreground">{accountEmail}</span>
        </p>
      )}
      <form action="/api/auth/logout" method="post">
        <button
          type="submit"
          className="text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:text-foreground hover:decoration-foreground"
        >
          Sign out
        </button>
      </form>
    </div>
  );

  return (
    <div className="drawer lg:drawer-open">
      <input id={CONSOLE_NAV_TOGGLE_ID} type="checkbox" className="drawer-toggle" />

      <div className="drawer-content flex min-h-dvh flex-col">
        {/* Slim sticky top bar — visible at every breakpoint; carries the
            hamburger below `lg`, wordmark always. §6.2's own sketch also
            names a search field and account control living here; neither
            exists yet (no search backend, and the account control already
            lives in the sidebar's own footer per this file's accountSlot
            above) — left for a later screen agent to add, not stubbed. */}
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
          topItem={NAV_TOP}
          groups={NAV_GROUPS}
          footerItems={NAV_FOOTER}
          currentPath={pathname}
          accountSlot={accountSlot}
          className="min-h-dvh w-[260px] border-edge border-r lg:w-[64px] xl:w-[260px]"
        />
      </div>
    </div>
  );
}
