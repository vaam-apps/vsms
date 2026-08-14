"use client";

import { Disclosure, DisclosureButton, DisclosurePanel } from "@headlessui/react";
import { useMediaQuery } from "@uidotdev/usehooks";
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
 * One nav row. `compact` is the icon-only-rail treatment (§6.1's `lg` band,
 * 1024–1279px): the label collapses to screen-reader-only text and a
 * daisyUI `.tooltip` (D5) carries it back on hover/focus instead.
 */
function NavLink({
  item,
  active,
  compact,
  dim = false,
}: {
  item: NavItem;
  active: boolean;
  compact: boolean;
  dim?: boolean;
}) {
  const Icon = item.icon;
  return (
    <a
      href={item.href}
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex items-center gap-3 rounded-field px-3 py-2 transition-colors",
        dim ? "text-caption" : "text-body",
        compact && "tooltip tooltip-right justify-center px-0",
        active && !dim && "bg-base-300 font-medium text-foreground",
        active && dim && "text-foreground",
        !active && "text-muted-foreground hover:bg-base-300/60 hover:text-foreground",
      )}
      data-tip={compact ? item.label : undefined}
    >
      <Icon size={16} className="shrink-0" aria-hidden="true" />
      <span className={cn(compact && "sr-only")}>{item.label}</span>
    </a>
  );
}

/**
 * One group's worth of rows, in one of two shapes depending on breakpoint
 * (console-redesign.md §6.1, §4):
 *
 * - **Off-canvas** (`accordion`, <1024px — both the phone band and the
 *   tablet band that §6.1 explicitly keeps behind the hamburger): a
 *   Headless UI `Disclosure`, collapsed by default, expanded only if it
 *   contains the current route — "roughly 6–8 tappable rows on open, not
 *   18" (§4).
 * - **Persistent** (≥1024px, `compact` further distinguishing the
 *   1024–1279px icon rail from the ≥1280px full-label sidebar): every group
 *   always fully expanded, matching §1.1's LottieFiles reference, which
 *   shows no user-collapsible groups on desktop at all.
 */
function GroupSection({
  group,
  currentPath,
  compact,
  accordion,
}: {
  group: NavGroup;
  currentPath: string;
  compact: boolean;
  accordion: boolean;
}) {
  const hasActiveItem = group.items.some((item) => isActive(item.href, currentPath));

  if (accordion) {
    return (
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
                  compact={false}
                />
              ))}
            </DisclosurePanel>
          </div>
        )}
      </Disclosure>
    );
  }

  return (
    <div className="flex flex-col gap-0.5">
      {compact ? (
        <div className="mx-3 my-1 border-edge-subtle border-t" aria-hidden="true" />
      ) : (
        <p className="px-3 py-1.5 text-caption text-subtle-foreground uppercase tracking-wide">
          {group.label}
        </p>
      )}
      {group.items.map((item) => (
        <NavLink
          key={item.href}
          item={item}
          active={isActive(item.href, currentPath)}
          compact={compact}
        />
      ))}
    </div>
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
 * match it):
 *   - `<1024px` (phone and tablet alike): off-canvas, full labels,
 *     collapsible accordion groups.
 *   - `1024–1279px`: persistent icon-only rail, tooltip on hover/focus.
 *   - `≥1280px`: persistent full sidebar with labels.
 */
export function SideNav({
  topItem,
  groups,
  footerItems,
  currentPath,
  accountSlot,
  className,
}: SideNavProps) {
  const isOffCanvas = useMediaQuery("(max-width: 1023px)");
  const isRail = useMediaQuery("(min-width: 1024px) and (max-width: 1279px)");
  const compact = !isOffCanvas && isRail;

  return (
    <nav
      aria-label="Primary"
      className={cn("flex h-full flex-col gap-4 overflow-y-auto bg-base-200 py-4", className)}
    >
      <div className="flex flex-col gap-0.5 px-2">
        <NavLink item={topItem} active={isActive(topItem.href, currentPath)} compact={compact} />
      </div>

      <div className="flex flex-1 flex-col gap-4 px-2">
        {groups.map((group) => (
          <GroupSection
            key={group.label}
            group={group}
            currentPath={currentPath}
            compact={compact}
            accordion={isOffCanvas}
          />
        ))}
      </div>

      <div className="flex flex-col gap-2 border-edge-subtle border-t px-2 pt-3">
        <div className="flex flex-col gap-0.5">
          {footerItems.map((item) => (
            <NavLink
              key={item.href}
              item={item}
              active={isActive(item.href, currentPath)}
              compact={compact}
              dim
            />
          ))}
        </div>
        {accountSlot != null && !compact && <div className="px-3">{accountSlot}</div>}
      </div>
    </nav>
  );
}
