// Pure module, extracted verbatim out of `workers-screen.tsx` (R6, AGENTS.md
// "Extracted pure modules carry tests" — this one has a test:
// `role-labels.test.ts`, same directory).
//
// The six §7.1 roles (`backends/crates/sms-worker`'s own `Role` enum), not
// derived from any generated type — `workers.locks`'s own `role` field is a
// plain `string` on the wire (`workers-screen.tsx`'s own
// `WorkerLockInfo["role"]`), so there is nothing to exhaustively match
// against at the type level the way `JOB_STATUS_META`'s `Record<JobState,
// ...>` can. `roleLabel` falls back to the raw role string for anything
// this table doesn't recognise, deliberately: a seventh role landing on the
// backend before this file is updated should still render *something*
// readable, not disappear or throw.

export const ROLE_LABELS: Record<string, string> = {
  dispatch: "Dispatch",
  drain: "Drain",
  scheduler: "Scheduler",
  hooks: "Hooks",
  jobs: "Jobs",
  smpp: "SMPP",
};

export function roleLabel(role: string): string {
  return ROLE_LABELS[role] ?? role;
}
