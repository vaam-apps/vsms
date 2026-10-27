// The console's information architecture (docs/design/console-redesign.md
// §4), as data — `@vaam-apps/ui`'s `SideNav` (§6.2) renders it, and owns nothing
// about *which* routes exist or how they're grouped. Eighteen route
// directories, grouped per the operator's own mental model, not the
// schema (§4's own rationale): MESSAGING is what you touch to send and
// watch traffic; DELIVERY is the infrastructure that decides how a message
// leaves; OPERATIONS is the worker/job/opt-out machinery that keeps the
// system healthy; ADMIN is account/access/compliance surface.
//
// `/pair` is deliberately absent — it's the pre-pairing page with no shell
// (§4), and `console-shell.tsx` renders it bare rather than looking it up
// here. `/api/*` is not a page.

import type { NavGroup, NavItem } from "@vaam-apps/ui";
import {
  Cpu,
  Fingerprint,
  FlaskConical,
  KeyRound,
  LayoutDashboard,
  LayoutGrid,
  ListChecks,
  MessageSquare,
  Route,
  ScrollText,
  Server,
  Settings,
  SquarePen,
  Users,
  UserX,
  Webhook,
} from "lucide-react";

/** Flat, ungrouped, always first (§4). The dashboard is the console's
 * landing page: an operator opens the console to see whether traffic is
 * flowing before they do anything else. It lived at `/dashboard` with the
 * composer at `/`; `/dashboard` still answers, with a redirect to `/`. */
export const NAV_TOP: NavItem = { label: "Dashboard", href: "/", icon: LayoutDashboard };

export const NAV_GROUPS: NavGroup[] = [
  {
    label: "Messaging",
    items: [
      { label: "Composer", href: "/composer", icon: SquarePen },
      { label: "Messages", href: "/messages", icon: MessageSquare },
      { label: "Simulator", href: "/simulator", icon: FlaskConical },
    ],
  },
  {
    label: "Delivery",
    items: [
      { label: "Providers", href: "/providers", icon: Server },
      { label: "Routes", href: "/routes", icon: Route },
      { label: "Sender IDs", href: "/sender-ids", icon: Fingerprint },
      { label: "Webhooks", href: "/webhooks", icon: Webhook },
    ],
  },
  {
    label: "Operations",
    items: [
      { label: "Jobs", href: "/jobs", icon: ListChecks },
      { label: "Workers", href: "/workers", icon: Cpu },
      { label: "Opt-outs", href: "/opt-outs", icon: UserX },
    ],
  },
  {
    label: "Admin",
    items: [
      { label: "Apps", href: "/apps", icon: LayoutGrid },
      { label: "Users & roles", href: "/users", icon: Users },
      { label: "Audit log", href: "/audit-log", icon: ScrollText },
      { label: "This console", href: "/console", icon: KeyRound },
      { label: "Settings", href: "/settings", icon: Settings },
    ],
  },
];

/** De-emphasized footer utility row (§4: "dev-only, de-emphasized like
 * 'Invite members'") — the account row (this console's key, and the way to its
 * lifecycle screen) is not data, it depends on the key's status, so it is built
 * directly in `console-shell.tsx` instead of listed here. */
export const NAV_FOOTER: NavItem[] = [];

/** Where a path sits in the navigation: its group's label (absent for the
 * top item) and its own label. The longest matching `href` wins, so
 * `/messages/abc` is "Messages" and `/` matches only itself. `undefined`
 * for a path the navigation does not list. Pure, for the top app bar. */
export function locate(pathname: string): { section?: string; page: string } | undefined {
  const matches = (href: string) =>
    href === "/" ? pathname === "/" : pathname === href || pathname.startsWith(`${href}/`);
  let best: { section?: string; page: string; length: number } | undefined;
  const consider = (item: NavItem, section?: string) => {
    if (!matches(item.href)) return;
    if (best !== undefined && best.length >= item.href.length) return;
    best =
      section === undefined
        ? { page: item.label, length: item.href.length }
        : { section, page: item.label, length: item.href.length };
  };
  consider(NAV_TOP);
  for (const group of NAV_GROUPS) for (const item of group.items) consider(item, group.label);
  for (const item of NAV_FOOTER) consider(item);
  if (best === undefined) return undefined;
  const { length: _length, ...rest } = best;
  return rest;
}
