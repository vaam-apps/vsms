/**
 * Every OAuth scope this deployment's server actually enforces.
 *
 * # How this list was derived, and why that matters
 *
 * Not from `docs/architecture.md` §5.2, and not from memory — from the
 * literals the Rust side really checks:
 *
 * ```
 * grep -rhoE 'require_permission\([^,]+, *"[a-z:]+"' backends/crates/ --include='*.rs'
 * grep -rhoE '"[a-z]+:[a-z]+"' backends/crates/sms-api/src/router.rs
 * ```
 *
 * That union is the set below. Two near-misses were checked and
 * deliberately excluded, because offering a scope nothing enforces is
 * worse than offering none — it implies a control that does not exist:
 *
 * - `message:send` appears only as **test fixture data** in `rbac.rs`'s
 *   own unit tests. AGENTS.md records the seeded role permissions being
 *   renamed `message:send` -> `sms:send` precisely because the literals
 *   had drifted from what `require_permission` checks.
 * - `provider:write` appears only inside a **doc comment** in `router.rs`
 *   explaining a past bug — the constant once checked that literal, which
 *   matched nothing, permanently denying a legitimate operator token.
 *
 * # This list can still drift
 *
 * Nothing mechanically ties it to the Rust literals; a new
 * `require_permission("thing:do")` will not appear here on its own. That
 * is the same shape as every other duplicated-vocabulary bug this repo has
 * hit, and it wants an `xtask` parity check of the kind that already
 * guards the state machines. Not built here — flagged, so the next person
 * adding a scope knows there are two places.
 */
export const KNOWN_SCOPES = [
  "sms:send",
  "sms:read",
  "job:read",
  "job:enqueue",
  "worker:read",
  "provider:read",
  "provider:update",
  "route:read",
  "sender:manage",
  "webhook:manage",
  "optout:manage",
  "dashboard:read",
  "audit:read",
  "user:manage",
] as const;

export type KnownScope = (typeof KNOWN_SCOPES)[number];

/** What each scope actually permits, in the console's own words. */
export const SCOPE_DESCRIPTIONS: Record<KnownScope, string> = {
  "sms:send": "Send messages",
  "sms:read": "Read messages and delivery receipts",
  "job:read": "Read the job queue",
  "job:enqueue": "Re-enqueue a dead job",
  "worker:read": "Read worker leases",
  "provider:read": "Read providers",
  "provider:update": "Edit providers",
  "route:read": "Read routes and simulate routing",
  "sender:manage": "Create and edit sender IDs",
  "webhook:manage": "Manage webhook endpoints, rotate secrets, replay",
  "optout:manage": "Record and remove opt-outs",
  "dashboard:read": "Read dashboard summaries",
  "audit:read": "Read the audit log",
  "user:manage": "Manage users and roles",
};

/**
 * The scopes a typical machine client needs — what `scripts/demo.sh` and
 * the getting-started runbook provision the console's own client with.
 * Offered as a one-click starting point rather than making an operator
 * pick fourteen checkboxes to get the common case.
 */
export const DEFAULT_CLIENT_SCOPES: readonly KnownScope[] = ["sms:send", "sms:read"];

/** Parse the space-delimited form the API stores into known scopes. */
export function parseScopes(raw: string): KnownScope[] {
  const known = new Set<string>(KNOWN_SCOPES);
  return raw
    .split(/\s+/)
    .filter((s) => known.has(s))
    .map((s) => s as KnownScope);
}

/** Serialise back to the space-delimited form the API expects. */
export function serializeScopes(scopes: readonly KnownScope[]): string {
  // Emitted in KNOWN_SCOPES order rather than click order, so the stored
  // value is stable regardless of the sequence an operator ticked boxes in
  // — otherwise two identical grants diff against each other.
  return KNOWN_SCOPES.filter((s) => scopes.includes(s)).join(" ");
}
