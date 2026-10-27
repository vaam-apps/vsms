"use client";

// The console's app shell: mounted once from `layout.tsx` (inside the gate), it
// decides whether to draw any chrome at all (not on `/pair`, a pre-pairing page
// with no navigation to offer) and builds the data the dumb chrome components
// need: the current path, the page's place in the navigation for the top bar,
// the theme, and the account block. Every class lives in `./components/` (R6).

import { useTheme } from "@vaam-apps/ui";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { AccountSlot } from "./components/account-slot";
import { ConsoleChrome } from "./components/console-chrome";
import { TopAppBar } from "./components/top-app-bar";
import { PAIR_PATH } from "./console-gate";
import { useKeyStatus } from "./key-status";
import { summarizeKey } from "./key-summary";
import { locate, NAV_FOOTER, NAV_GROUPS, NAV_TOP } from "./nav-groups";
import { useClientLinks } from "./use-client-links";

export function ConsoleShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const { preference, resolvedTheme, setPreference } = useTheme();
  const { status } = useKeyStatus();
  // Every plain `<a href>` in the console, `SideNav`'s rows included,
  // navigates client-side instead of reloading the document
  // (`client-links.ts`).
  useClientLinks();

  if (pathname === PAIR_PATH) {
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
          keySummary={summarizeKey(status, Date.now())}
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
