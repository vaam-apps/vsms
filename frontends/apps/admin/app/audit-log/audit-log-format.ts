// Pure formatting helper for the Audit log screen (R6: extracted pure
// modules carry tests — see `audit-log-format.test.ts`).
//
// Every entry's `primaryKey`/`actor`/`before`/`after` is JSON-encoded text,
// not parsed further by `@vsms/gateway` (that module's own doc: "the same
// convention `Provider.config`/`Route.config` already use for a JSON-shaped
// `String` column"). Pretty-prints when it parses; falls back to the raw
// string otherwise rather than hiding a value this screen can't make sense
// of — an audit trail should never quietly drop something it couldn't
// format.
export function prettyJson(raw: string | undefined): string | undefined {
  if (raw === undefined) return undefined;
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
