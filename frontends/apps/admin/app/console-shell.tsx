"use client";

// The console's app shell (docs/design/console-redesign.md §6.2, Phase 0).
// Replaces `console-nav.tsx`'s per-screen header pattern with one shared
// chrome mounted once from `frontends/apps/admin/app/layout.tsx` — see that file's own
// module doc for what a per-screen re-skin must NOT change about data
// fetching (messages-screen.tsx's live-poll loop, AsyncLocalStorage token
// forwarding, Layer-2 403 surfacing, nuqs URL state, the OIDC session
// handling — none of that lives here or is touched by this file).
//
// `console-nav.tsx` no longer exists — a grep across the whole tree found
// zero imports and zero JSX usages of it (the two screens whose own
// comments once described using it, `opt-outs-screen.tsx`/
// `settings-screen.tsx`, already say they dropped it in favour of this
// shell's sidebar), so it was deleted rather than left as dead code this
// PR's own R6 audit would otherwise have to explain away.
//
// # R6
//
// This file is the smart half of the shell: it decides *whether* to render
// chrome at all (the `/login` bare-render case) and builds the data
// `ConsoleChrome`/`AccountSlot` need (`usePathname()`, the account email).
// Every class and every piece of markup that used to live here now lives
// in `./components/console-chrome.tsx` and `./components/account-slot.tsx`.

import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { AccountSlot } from "./components/account-slot";
import { ConsoleChrome } from "./components/console-chrome";
import { NAV_FOOTER, NAV_GROUPS, NAV_TOP } from "./nav-groups";

export function ConsoleShell({
  children,
  accountEmail,
}: {
  children: ReactNode;
  /** The signed-in human's email, read server-side in `layout.tsx` off the
   * `x-vsms-actor` header `frontends/apps/admin/middleware.ts` already sets on every
   * authenticated request (the same header `frontends/packages/api/src/context.ts`
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

  return (
    <ConsoleChrome
      topItem={NAV_TOP}
      groups={NAV_GROUPS}
      footerItems={NAV_FOOTER}
      currentPath={pathname}
      accountSlot={<AccountSlot email={accountEmail} />}
    >
      {children}
    </ConsoleChrome>
  );
}
