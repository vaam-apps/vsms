"use client";

// A small shared nav strip for #52/#58's five new screens (Apps, Users,
// Opt-outs, Audit log, Settings) — every existing screen in this console
// hand-rolls its own `<header>` nav block with its own subset of links
// (`providers-screen.tsx`/`jobs-screen.tsx`/etc. each pick a different
// handful), so there's no shared component to extend without touching
// every one of them. Rather than repeat the same five-link block five
// times across these new files, or take on the blast radius of editing
// every pre-existing screen's own header just to add five more links each,
// this factors the *new* screens' shared nav into one place — still plain,
// still hand-rolled, just not copy-pasted five times over. Existing
// screens are left exactly as they were; `dashboard-screen.tsx` (the
// console's own hub) separately gained links to these five so they're
// reachable by click, not only by typing a URL.

import { ThemeToggle } from "@vsms/ui";

const LINKS: { href: string; label: string }[] = [
  { href: "/dashboard", label: "Dashboard" },
  { href: "/apps", label: "Apps" },
  { href: "/users", label: "Users & roles" },
  { href: "/opt-outs", label: "Opt-outs" },
  { href: "/audit-log", label: "Audit log" },
  { href: "/settings", label: "Settings" },
  { href: "/", label: "Composer" },
];

export function ConsoleNav({ current }: { current: string }) {
  return (
    <div className="flex shrink-0 items-center gap-3">
      {LINKS.filter((link) => link.href !== current).map((link) => (
        <a
          key={link.href}
          href={link.href}
          className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
        >
          {link.label}
        </a>
      ))}
      <ThemeToggle />
    </div>
  );
}
