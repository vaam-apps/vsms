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

/**
 * A one-line reading of an audit row's `actor` snapshot for the table:
 * `role · subject` (a human: their role and user id; a machine: `app` or
 * `system` and its client or subsystem). The raw snapshot is JSON the
 * framework writes; its claims sit either at the top level or under
 * `claims`, so both are read. Anything unparseable is shown as it came,
 * which is what the table showed for every row before this.
 */
export function actorLabel(raw: string): string {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return raw;
  }
  if (parsed === null || typeof parsed !== "object") return raw;
  const top = parsed as Record<string, unknown>;
  const claims =
    top.claims !== null && typeof top.claims === "object"
      ? (top.claims as Record<string, unknown>)
      : top;
  const pick = (key: string): string | undefined => {
    const value = claims[key] ?? top[key];
    return typeof value === "string" && value !== "" ? value : undefined;
  };
  const who = pick("role") ?? pick("kind");
  const sub = pick("sub");
  if (who === undefined && sub === undefined) return raw;
  return [who, sub].filter((part) => part !== undefined).join(" · ");
}

/** A primary-key snapshot without its JSON string quotes, when it is one. */
export function primaryKeyLabel(raw: string): string {
  try {
    const parsed: unknown = JSON.parse(raw);
    return typeof parsed === "string" ? parsed : raw;
  } catch {
    return raw;
  }
}
