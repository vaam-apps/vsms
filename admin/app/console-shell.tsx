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

import type { SideNavProps } from "@vsms/ui";
import { Menu } from "lucide-react";
import dynamic from "next/dynamic";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { NAV_FOOTER, NAV_GROUPS, NAV_TOP } from "./nav-groups";

const CONSOLE_NAV_TOGGLE_ID = "console-nav";

// Found live (a real 500, not a lint/typecheck finding — `pnpm build` and
// `tsc` both stay green through this): `@vsms/ui`'s `SideNav` calls
// `@uidotdev/usehooks`' `useMediaQuery` (D12), and that hook's own
// `getServerSnapshot` is a hard `throw new Error("useMediaQuery is a
// client-only hook")` — read directly from
// `@uidotdev/usehooks/index.js`, not inferred. `SideNav` still renders
// during SSR by default (a `"use client"` component is not exempt from
// the server render pass, only from server-only APIs), so every full page
// load under this shell 500'd until this was found. `next/dynamic` with
// `ssr: false` is the fix, not a workaround: it's the one thing that
// stops React from ever invoking the hook's `getServerSnapshot` in the
// first place, on either the server render or the initial client
// hydration pass (so no hydration mismatch either) — a plain conditional
// render (e.g. gating on `@uidotdev/usehooks`' own `useIsClient()`) can't
// help here, since the crash happens the instant `useMediaQuery` is
// *called*, regardless of how its return value is used afterward. This
// has to live here, in `admin/`, not inside `@vsms/ui` itself —
// `next/dynamic` is a Next.js API and `packages/ui` has no dependency on
// Next (by design, so it stays framework-agnostic beyond React). Any
// other Phase 1/2 component reaching for `useMediaQuery` (D12 also names
// it for `LiveRow`'s reduced-motion check) needs the identical treatment
// if it can be part of an initial server-rendered page.
const SideNav = dynamic<SideNavProps>(() => import("@vsms/ui").then((mod) => mod.SideNav), {
  ssr: false,
  loading: () => (
    <div className="min-h-dvh w-[260px] border-edge border-r bg-base-200 lg:w-[64px] xl:w-[260px]" />
  ),
});

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
