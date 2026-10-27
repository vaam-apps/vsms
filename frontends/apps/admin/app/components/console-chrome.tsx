// Dumb component (R6): the console's frame — the navigation, the top app
// bar and the content column. `console-shell.tsx` decides whether to render
// it (not on `/pair`) and hands it the data; every class lives here.
//
// # Why the floating `SideNav`, not the drawer it replaced
//
// The shell used to own a daisyUI drawer and pass `smallScreen="off-canvas"`,
// so a phone got a hamburger, a full-height sheet of eighteen rows, and a
// sign-out link at the very bottom of it. `@vaam-apps/ui`'s default mode is
// Material 3 Expressive's navigation as the platform draws it: a floating
// toolbar along the bottom below 640px (four destinations and a More sheet
// holding the rest and the account block), a floating vertical toolbar from
// 640px, and the full sidebar from 1280px. The account block is reachable at
// every width without a drawer of our own, which is the one thing the old
// shell could not do in the 1024-1279px band.
//
// The rails are `position: fixed` and portalled to `document.body`, so they
// cannot reserve their own space: the paddings on the top bar and on `main`
// are this file's job (`primitives-layout.md`, "Only the sidebar takes space
// out of the page"). Below 640px `pb-28` clears the 80px bottom toolbar;
// from 640px `sm:pl-24` clears the 80px vertical one; from 1280px the
// sidebar is in flow and both go back to the normal gutter.
//
// The sidebar is `sticky` at full viewport height so it does not scroll away
// with a long table. No `display` class is passed to `SideNav`: its own
// `hidden` below 1280px is what keeps a second "Primary" landmark out of the
// accessibility tree.

import { type NavGroup, type NavItem, SideNav } from "@vaam-apps/ui";
import type { ReactNode } from "react";

export interface ConsoleChromeProps {
  topItem: NavItem;
  groups: NavGroup[];
  footerItems: NavItem[];
  currentPath: string;
  accountSlot: ReactNode;
  topBar: ReactNode;
  children: ReactNode;
}

export function ConsoleChrome({
  topItem,
  groups,
  footerItems,
  currentPath,
  accountSlot,
  topBar,
  children,
}: ConsoleChromeProps) {
  return (
    <div className="flex min-h-dvh bg-base-100">
      <SideNav
        topItem={topItem}
        groups={groups}
        footerItems={footerItems}
        currentPath={currentPath}
        accountSlot={accountSlot}
        className="xl:sticky xl:top-0 xl:h-dvh xl:w-72 xl:bg-surface-container-low xl:pt-3"
      />
      <div className="flex min-w-0 flex-1 flex-col">
        {topBar}
        <main
          id="main"
          className="mx-auto w-full min-w-0 max-w-[1440px] flex-1 px-4 pt-2 pb-28 sm:pr-6 sm:pb-10 sm:pl-24 xl:px-10"
        >
          {children}
        </main>
      </div>
    </div>
  );
}
