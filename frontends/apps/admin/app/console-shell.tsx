"use client";

// The console's app shell: mounted once from `layout.tsx`, it decides
// whether to draw any chrome at all (not on `/login`, a pre-auth gate) and
// builds the data the dumb chrome components need: the current path, the
// page's place in the navigation for the top bar, the theme, and the
// account block. Every class lives in `./components/` (R6).

import { useTheme } from "@vaam-apps/ui";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { AccountSlot } from "./components/account-slot";
import { ConsoleChrome } from "./components/console-chrome";
import { TopAppBar } from "./components/top-app-bar";
import { locate, NAV_FOOTER, NAV_GROUPS, NAV_TOP } from "./nav-groups";
import { useClientLinks } from "./use-client-links";

export function ConsoleShell({
  children,
  accountEmail,
}: {
  children: ReactNode;
  /** The signed-in human's email, read server-side in `layout.tsx` off the
   * `x-vsms-actor` header `middleware.ts` sets on every authenticated
   * request. Display only, not an auth check: `middleware.ts` has already
   * decided whether this request may render. */
  accountEmail?: string | null;
}) {
  const pathname = usePathname();
  const { preference, resolvedTheme, setPreference } = useTheme();
  // Every plain `<a href>` in the console, `SideNav`'s rows included,
  // navigates client-side instead of reloading the document
  // (`client-links.ts`).
  useClientLinks();

  if (pathname === "/login") {
    return <>{children}</>;
  }

  const here = locate(pathname);
  return (
    <ConsoleChrome
      topItem={NAV_TOP}
      groups={NAV_GROUPS}
      footerItems={NAV_FOOTER}
      currentPath={pathname}
      accountSlot={
        <AccountSlot
          email={accountEmail}
          themePreference={preference}
          onThemePreferenceChange={setPreference}
        />
      }
      topBar={
        <TopAppBar
          section={here?.section}
          page={here?.page}
          resolvedTheme={resolvedTheme}
          onToggleTheme={() => setPreference(resolvedTheme === "dark" ? "light" : "dark")}
        />
      }
    >
      {children}
    </ConsoleChrome>
  );
}
