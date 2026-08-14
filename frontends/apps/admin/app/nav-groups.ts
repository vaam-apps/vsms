// The console's information architecture (docs/design/console-redesign.md
// §4), as data — `@vsms/ui`'s `SideNav` (§6.2) renders it, and owns nothing
// about *which* routes exist or how they're grouped. Eighteen route
// directories, grouped per the operator's own mental model, not the
// schema (§4's own rationale): MESSAGING is what you touch to send and
// watch traffic; DELIVERY is the infrastructure that decides how a message
// leaves; OPERATIONS is the worker/job/opt-out machinery that keeps the
// system healthy; ADMIN is account/access/compliance surface.
//
// `/login` is deliberately absent — it's a pre-auth gate with no shell
// (§4), and `console-shell.tsx` renders it bare rather than looking it up
// here. `/api/*` is not a page.

import type { NavGroup, NavItem } from "@vsms/ui";
import {
  Component,
  Cpu,
  Fingerprint,
  FlaskConical,
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

/** Flat, ungrouped, always first (§4). */
export const NAV_TOP: NavItem = { label: "Dashboard", href: "/dashboard", icon: LayoutDashboard };

export const NAV_GROUPS: NavGroup[] = [
  {
    label: "Messaging",
    items: [
      { label: "Composer", href: "/", icon: SquarePen },
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
      { label: "Settings", href: "/settings", icon: Settings },
    ],
  },
];

/** De-emphasized footer utility row (§4: "dev-only, de-emphasized like
 * 'Invite members'") — the account row (§4's "Signed in as <email> · Sign
 * out") is not data, it depends on the signed-in session, so it is built
 * directly in `console-shell.tsx` instead of listed here. */
export const NAV_FOOTER: NavItem[] = [
  { label: "Component gallery", href: "/gallery", icon: Component },
];
