"use client";

import { Disclosure, DisclosureButton, DisclosurePanel } from "@headlessui/react";
import { ChevronDown } from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * The console's information architecture (console-redesign.md §4), as data.
 * `@vsms/ui` owns the shape; `admin/app/nav-groups.ts` owns the actual
 * content (routes, groupings, icons) — this file never hardcodes a route.
 */
export interface NavItem {
  label: string;
  href: string;
  /** Any lucide-react icon component. Typed structurally so this file
   * doesn't need to depend on `lucide-react`'s own exported icon type name. */
  icon: ComponentType<{ size?: number; className?: string; "aria-hidden"?: boolean | "true" }>;
}

export interface NavGroup {
  /** Small-caps section header, e.g. "Messaging" (console-redesign.md §4). */
  label: string;
  items: NavItem[];
}

export interface SideNavProps {
  /** Flat, ungrouped, always first — §4 ("Dashboard"). */
  topItem: NavItem;
  groups: NavGroup[];
  /** De-emphasized footer utility rows (§1.1: "administrivia, not content"). */
  footerItems: NavItem[];
  /** Current pathname, for active-row highlighting. Plain string, not a
   * `next/navigation` call — this package has no dependency on Next.js, so
   * the caller (`admin/app/console-shell.tsx`) resolves it via
   * `usePathname()` and passes it down. */
  currentPath: string;
  /** App-specific account/sign-out markup (§4's footer "Signed in as
   * <email> · Sign out" row) — rendered as-is, never built here, so this
   * component stays free of any auth-specific knowledge. */
  accountSlot?: ReactNode;
  className?: string;
}

function isActive(href: string, currentPath: string): boolean {
  if (href === "/") return currentPath === "/";
  return currentPath === href || currentPath.startsWith(`${href}/`);
}

/**
 * One nav row.
 *
 * **CSS-driven, not `useMediaQuery`-driven — found live, corrected after
 * an earlier revision of this file used `@uidotdev/usehooks`' hook here
 * and wrapped `SideNav` in `next/dynamic({ ssr: false })` to work around
 * its SSR crash (see `console-shell.tsx`'s own history / this file's own
 * git log for the full finding).** That fix traded a real cost — the
 * sidebar never appeared in the server-rendered HTML, so every full page
 * load painted an empty slab and popped the real nav in after hydration —
 * for a problem the breakpoint switch itself never needed JS to solve:
 * `variant="responsive"` below expresses "full label off-canvas and at
 * `xl`, `sr-only` at the `lg` icon-rail band" purely with `lg:sr-only
 * xl:not-sr-only`, no hook, no client-only state, no hydration boundary.
 *
 * - **`"responsive"`**: the label is visible off-canvas (`<1024px`, where
 *   this row is never reached without the group around it already being
 *   fully expanded) and at `≥1280px`, `sr-only` in the `1024–1279px` icon
 *   rail band — a daisyUI `.tooltip`/`data-tip` (D5) carries the label
 *   back on hover/focus there. Used for every row that can appear in the
 *   *persistent* sidebar: `topItem`, `footerItems`, and each group's own
 *   "always expanded" desktop rows (`GroupSection` below).
 * - **`"always"`**: the label is always visible, no responsive classes at
 *   all. Used only inside the off-canvas accordion tree, which is itself
 *   gated to `<1024px` by its own `lg:hidden` wrapper — a per-row
 *   breakpoint switch would be redundant there, since that whole tree
 *   never renders at any width where a switch could matter.
 */
function NavLink({
  item,
  active,
  variant,
  dim = false,
}: {
  item: NavItem;
  active: boolean;
  variant: "responsive" | "always";
  dim?: boolean;
}) {
  const Icon = item.icon;
  const responsive = variant === "responsive";
  return (
    <a
      href={item.href}
      aria-current={active ? "page" : undefined}
      data-tip={item.label}
      className={cn(
        "flex items-center gap-3 rounded-field px-3 py-2 transition-colors",
        dim ? "text-caption" : "text-body",
        responsive && "tooltip tooltip-right lg:justify-center lg:px-0 xl:justify-start xl:px-3",
        active && !dim && "bg-base-300 font-medium text-foreground",
        active && dim && "text-foreground",
        !active && "text-muted-foreground hover:bg-base-300/60 hover:text-foreground",
      )}
    >
      <Icon size={16} className="shrink-0" aria-hidden="true" />
      <span className={cn(responsive && "lg:sr-only xl:not-sr-only")}>{item.label}</span>
    </a>
  );
}

/**
 * One group's worth of rows, rendered as **two parallel trees**, one per
 * §6.1 regime, each visible only at its own breakpoint via a plain
 * `hidden`/`lg:hidden` toggle — not one tree whose *content* changes based
 * on a JS-read viewport width. The two regimes genuinely need different
 * DOM, not just different visibility, because the off-canvas tree is
 * interactive (a collapsible `Disclosure`) and the persistent tree is not
 * (every group is always fully expanded there, matching §1.1's LottieFiles
 * reference, which shows no user-collapsible groups on desktop at all) —
 * duplicating ~4 rows per group across two trees is a irrelevant DOM cost
 * next to the alternative (a client-only nav that never appears in the
 * first paint).
 *
 * - **Off-canvas** (`lg:hidden`, so effectively `<1024px` — both the phone
 *   band and the tablet band §6.1 explicitly keeps behind the hamburger):
 *   a Headless UI `Disclosure`, collapsed by default, expanded only if it
 *   contains the current route — "roughly 6–8 tappable rows on open, not
 *   18" (§4). `defaultOpen` is computed from `currentPath`, which is
 *   available identically on the server and the client, so this renders
 *   correctly on first paint with no hydration mismatch.
 * - **Persistent** (`hidden lg:flex`, so `≥1024px`): always fully
 *   expanded. The header/rail-divider split (small-caps text at `≥1280px`,
 *   a hairline divider instead in the `1024–1279px` icon rail band) is the
 *   same CSS-toggle technique `NavLink`'s own `"responsive"` variant uses.
 */
function GroupSection({ group, currentPath }: { group: NavGroup; currentPath: string }) {
  const hasActiveItem = group.items.some((item) => isActive(item.href, currentPath));

  return (
    <>
      <div className="lg:hidden">
        <Disclosure defaultOpen={hasActiveItem}>
          {({ open }) => (
            <div className="flex flex-col gap-0.5">
              <DisclosureButton className="flex items-center justify-between rounded-field px-3 py-1.5 text-left text-caption text-subtle-foreground uppercase tracking-wide hover:text-foreground">
                {group.label}
                <ChevronDown
                  size={14}
                  aria-hidden="true"
                  className={cn("transition-transform", open && "rotate-180")}
                />
              </DisclosureButton>
              <DisclosurePanel className="flex flex-col gap-0.5">
                {group.items.map((item) => (
                  <NavLink
                    key={item.href}
                    item={item}
                    active={isActive(item.href, currentPath)}
                    variant="always"
                  />
                ))}
              </DisclosurePanel>
            </div>
          )}
        </Disclosure>
      </div>

      <div className="hidden lg:flex lg:flex-col lg:gap-0.5">
        <p className="hidden px-3 py-1.5 text-caption text-subtle-foreground uppercase tracking-wide xl:block">
          {group.label}
        </p>
        <div className="mx-3 my-1 border-edge-subtle border-t xl:hidden" aria-hidden="true" />
        {group.items.map((item) => (
          <NavLink
            key={item.href}
            item={item}
            active={isActive(item.href, currentPath)}
            variant="responsive"
          />
        ))}
      </div>
    </>
  );
}

/**
 * The console's side navigation (console-redesign.md §4, §6.2). A pure
 * structural/layout component — routing, session data, and the drawer's
 * own off-canvas/persistent CSS split all live at the call site
 * (`admin/app/console-shell.tsx`, D7): this component only ever decides
 * "flat top item → grouped middle → de-emphasized footer" composition and
 * the label/icon-only/accordion treatment per breakpoint.
 *
 * Breakpoint behaviour (§6.1 is the authoritative table here — see that
 * section's own reasoning for why it supersedes §4's looser "Tablet
 * (768–1024px)" prose, which pre-dates that table and was never updated to
 * match it) — entirely CSS-driven (`lg:`/`xl:` Tailwind variants), never a
 * JS breakpoint read:
 *   - `<1024px` (phone and tablet alike): off-canvas, full labels,
 *     collapsible accordion groups.
 *   - `1024–1279px`: persistent icon-only rail, tooltip on hover/focus.
 *   - `≥1280px`: persistent full sidebar with labels.
 *
 * This means the whole nav — every row, every group, every breakpoint's
 * shape — is present in the server-rendered HTML on first paint. No
 * `next/dynamic({ ssr: false })`, no client-only mount flash. Verify that
 * claim directly, not by trusting this comment: `curl` a page and grep the
 * raw HTML for a nav item's label.
 */
export function SideNav({
  topItem,
  groups,
  footerItems,
  currentPath,
  accountSlot,
  className,
}: SideNavProps) {
  return (
    <nav
      aria-label="Primary"
      className={cn("flex h-full flex-col gap-4 overflow-y-auto bg-base-200 py-4", className)}
    >
      <div className="flex flex-col gap-0.5 px-2">
        <NavLink item={topItem} active={isActive(topItem.href, currentPath)} variant="responsive" />
      </div>

      <div className="flex flex-1 flex-col gap-4 px-2">
        {groups.map((group) => (
          <GroupSection key={group.label} group={group} currentPath={currentPath} />
        ))}
      </div>

      <div className="flex flex-col gap-2 border-edge-subtle border-t px-2 pt-3">
        <div className="flex flex-col gap-0.5">
          {footerItems.map((item) => (
            <NavLink
              key={item.href}
              item={item}
              active={isActive(item.href, currentPath)}
              variant="responsive"
              dim
            />
          ))}
        </div>
        {/* Hidden in the icon rail band only (no room for the account
            block at 64px); shown off-canvas and at the full-label desktop
            width — same `lg:hidden xl:block` toggle technique as the rest
            of this file, not a JS breakpoint check. */}
        {accountSlot != null && <div className="px-3 lg:hidden xl:block">{accountSlot}</div>}
      </div>
    </nav>
  );
}
