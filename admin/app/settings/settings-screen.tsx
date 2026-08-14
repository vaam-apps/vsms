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
//   `crates/sms-api/src/consent.rs`) rather than a runtime-editable value
//   — that PR's own reasoning: "the next reader should see a value to
//   reconsider, not a rule to trust," and no `Settings` model exists to
//   make it editable without inventing one. Building a form for a value
//   the schema doesn't expose would either silently do nothing (no
//   backing write path) or require a real schema change this ticket does
//   not scope.
// - **`SMS_HASH_PEPPER`**, rate-limit budgets, idempotency TTLs, and every
//   other operational knob this deployment has are process environment
//   variables (`app/sms-gateway`'s own CLI flags/env), not database rows —
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

export function SettingsScreen() {
  return (
    <div className="flex flex-col gap-6">
      <div className="border-edge border-b pb-6">
        <h1 className="font-medium text-foreground text-title">Settings</h1>
        <p className="mt-1 max-w-xl text-body text-muted-foreground">
          There is no runtime-configurable settings model in this system today.
        </p>
      </div>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        This screen exists because #58 named &quot;settings&quot; as one of the remaining pages, not
        because there is a <span className="font-mono text-foreground">Settings</span> model to
        build a form around — there isn&apos;t one. Rather than invent switches that don&apos;t
        connect to anything, here is where the things a &quot;settings&quot; screen might otherwise
        show actually live today.
      </div>

      <div className="flex flex-col divide-y divide-edge rounded-sm border border-edge">
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">Marketing quiet hours</p>
          <p className="text-caption text-muted-foreground">
            A self-imposed policy constant (§10), not a database row —{" "}
            <span className="font-mono text-foreground">
              crates/sms-api/src/consent.rs::MARKETING_QUIET_HOURS_START_WAT
            </span>{" "}
            / <span className="font-mono text-foreground">_END_WAT</span>. Deliberately not
            runtime-editable: the source comment is explicit that "the next reader should see a
            value to reconsider, not a rule to trust."
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">MSISDN/body hashing pepper</p>
          <p className="text-caption text-muted-foreground">
            <span className="font-mono text-foreground">SMS_HASH_PEPPER</span>, a process
            environment variable the gateway validates at startup — never read or writable from this
            console. Rotating it does not rehash stored rows; see{" "}
            <span className="font-mono text-foreground">OPEN_QUESTIONS.md</span> §3.1.
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">Provider routing rules</p>
          <p className="text-caption text-muted-foreground">
            Configurable, and already has a real screen — see{" "}
            <a href="/routes" className="underline decoration-edge-strong underline-offset-2">
              Routes
            </a>{" "}
            and the{" "}
            <a href="/simulator" className="underline decoration-edge-strong underline-offset-2">
              route simulator
            </a>
            .
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">
            Rate limits, idempotency TTLs, and other operational knobs
          </p>
          <p className="text-caption text-muted-foreground">
            Process CLI flags/environment variables on{" "}
            <span className="font-mono text-foreground">sms-gateway serve</span>, set at deploy time
            — not database rows, and out of this console&apos;s reach by design.
          </p>
        </div>
      </div>
    </div>
  );
}
