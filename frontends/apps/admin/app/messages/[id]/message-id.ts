// Route-param validation for `/messages/[id]` (R6, AGENTS.md: "Pages
// validate their inputs"). `Cuid` is format-guarded `[a-z0-9]{2,32}` on
// every REST route that filters by id (`AGENTS.md`'s own "Framework
// constraints" table, and `@vsms/ui`'s `IdDisplay` doc comment) — a
// malformed id 400s server-side rather than ever resolving to a real row.
// Checking the same shape here, before rendering a screen, turns that into
// a clean 404 instead of a screen built around a request that could never
// have succeeded. Deliberately the same bound as the server's own guard
// (`{2,32}`, not stricter to e.g. exactly 23 chars for `cs_cuid()`'s own
// output length) — this must stay in lockstep with what the server will
// actually accept, not add a second, tighter opinion of its own.

const MESSAGE_ID_PATTERN = /^[a-z0-9]{2,32}$/;

export function isValidMessageId(id: string): boolean {
  return MESSAGE_ID_PATTERN.test(id);
}
