"use client";

// The Settings screen (#58) — deliberately informational, not a form.
//
// #58's own issue text lists "settings" alongside opt-outs/users/roles/
// audit log without saying what it means, and there is no `Settings`
// model anywhere in `schema.cstack` to build a screen around. Two things
// this system might call "settings" were checked before deciding what (if
// anything) to build here:
//
// - **Marketing quiet hours** (§10, #72) were deliberately made a
//   `pub const` (`MARKETING_QUIET_HOURS_START_WAT`/`_END_WAT`,
//   `backends/crates/sms-api/src/consent.rs`) rather than a runtime-editable value
//   — that PR's own reasoning: "the next reader should see a value to
//   reconsider, not a rule to trust," and no `Settings` model exists to
//   make it editable without inventing one. Building a form for a value
//   the schema doesn't expose would either silently do nothing (no
//   backing write path) or require a real schema change this ticket does
//   not scope.
// - **`SMS_HASH_PEPPER`**, rate-limit budgets, idempotency TTLs, and every
//   other operational knob this deployment has are process environment
//   variables (`backends/apps/sms-gateway`'s own CLI flags/env), not database rows —
//   correct by design (`pepper.rs`'s own doc: rotation is a deliberate,
//   infrequent operational act, not something a web form should make one
//   click away), but also not something this console can read or change
//   at runtime without SSH access to the process, which is out of scope
//   for an admin screen.
//
// So: this screen states plainly that there is nothing here to edit yet,
// names where each of the above actually lives, and does not invent a
// screen with switches that don't connect to anything — matching this
// project's own "no dormant code" convention applied to UI rather than
// server code.
//
// # Console redesign (Phase 2, Admin group)
//
// A visual pass only, no behavioural change: the per-screen `<header>` +
// `ConsoleNav` block is gone (`ConsoleShell`'s sidebar already carries
// every route this used to link), and the outer wrapper is a plain `<div>`
// rather than a second, nested `<main>` — `ConsoleShell` already renders
// one around every route's `children` (docs/design/console-redesign.md
// §6.2). This screen stays a `Page` (§3: purely informational, nothing
// with per-record state to peek at), so no drawer of either weight
// applies here.
//
// # R6 — no data to fetch, still a thin smart layer over a dumb view
//
// This screen genuinely has no data fetching, mutations, permissions or
// URL state to own — but R6's layer split still applies: the markup and
// classes live in `./components/settings-panel.tsx`, a dumb component with
// no props, and this file does nothing but render it. Kept as a distinct
// `-screen.tsx` file (rather than pointing `page.tsx` straight at the dumb
// component) for consistency with every other route in this console, and
// because a settings screen is exactly the kind of "informational today"
// page most likely to grow real state later.

import { SettingsPanel } from "./components/settings-panel";

export function SettingsScreen() {
  return <SettingsPanel />;
}
