import "server-only";

// #221: the single place this package converts sms-api's wire format into
// its own `field?: T | undefined` convention.
//
// Every module in this package used to carry its own `parseJsonBody`
// (`JSON.parse` behind a try/catch) and, in twelve of them, its own
// `normalize*` function converting JSON's explicit `null` — what sms-api
// actually sends for a nullable column with no value, never an omitted key
// — into `undefined`. That finding was made three separate times by three
// separate pieces of work, each live, each against a real screen: #54
// (`Route.match*`, `Provider.healthCheckedAt`, `SimulateRouteResult.tieBreak`
// — the last one crashed the simulator screen outright), #50
// (`Message.submittedAt` rendering a bogus Unix-epoch timeline entry), and
// #52/#58 (`User.lastLoginAt` rendering `1970-01-01`). Each fix landed as
// its own local `normalize*` function next to the module that found the
// bug — `routes.ts`, `providers.ts`, `route-simulator.ts`, `app-clients.ts`,
// `apps.ts`, `roles.ts`, `dashboard.ts`, `opt-outs.ts`, `users.ts`,
// `senders.ts` (two), `webhooks.ts` (two) — fourteen functions across twelve
// files by the time this PR started counting, not the five the issue was
// filed against. Two more modules, `jobs.ts` and `workers.ts`, had never
// been caught: `JobRecord.dedupeKey`/`leaseOwner`/`leaseUntil`/`lastError`/
// `startedAt`/`finishedAt` and `WorkerLockInfo.workerId`/`pid`/`heldSince`
// are exactly the same shape of nullable column, silently unguarded,
// because nothing had yet driven the Jobs or Workers screen through a case
// that renders one. That is the actual argument for consolidating here
// rather than writing a fifteenth, sixteenth, seventeenth `normalize*`: the
// next module forgets, and the forgetting is invisible until someone runs
// the right screen in a browser (which is exactly how all three prior
// instances were found, and why #221 was filed rather than patched a fourth
// time).
//
// # The hazard, and why a blanket recursive strip is safe here specifically
//
// A blind `null -> undefined` pass over every response would silently
// corrupt data if any response legitimately carries a JSON value where
// `null` is meaningful rather than "this column has no value" — most
// obviously, a stored JSON payload forwarded byte-for-byte. This was
// checked directly against `schemas/vsms.cstack` before writing a line of
// this function, not assumed: every field in this codebase that carries
// free-form or pre-serialised JSON — `WebhookAttempt.payload` (the signed
// envelope's `data`, forwarded exactly as stored per `hooks.rs`'s own doc —
// what gets signed must be exactly what gets sent), `cratestack_audit`'s
// own `actor`/`before`/`after`/`primaryKey` snapshot columns
// (`backends/crates/sms-api/src/audit_log.rs`), and `Provider.config` — is declared
// a plain `String` in `schema.cstack`, never a structured type. sms-api
// therefore always sends each of these as a JSON *string* value
// (`"{\"key\":\"value\"}"`), not as a nested object in the response tree.
// `JSON.parse` on the outer response body turns every one of them into a
// single opaque string leaf, and [`normalizeValue`] below only ever
// recurses into a plain object or an array — never into a string's own
// contents. These fields are therefore structurally unreachable by this
// walk regardless of what they encode, including a payload whose encoded
// text contains the literal substring `"null"`.
//
// That structural fact is the primary guarantee, and [`json.test.ts`]
// proves it against exactly that adversarial case. `VERBATIM_STRING_FIELDS`
// below is a second, named guard on top of it — defence in depth, not the
// mechanism itself — so that if a future schema change ever turned one of
// these columns into a genuinely structured (non-`String`) type, this walk
// still would not descend into it by name, rather than silently starting to
// the moment the structural argument above stopped holding. Update this set
// alongside any schema change that does that; it costs nothing to list a
// field that happens to already be a string leaf (skipping recursion into a
// value that was never an object or array is a no-op).
//
// `backends/crates/sms-api/src/route_simulator.rs`'s own `Decision` rendering
// (`route-simulator.ts`) and every other model's projected fields, by
// contrast, are ordinary structured JSON all the way down — arrays of
// objects, optional nested objects (`tieBreak`, `winner`) — so the walk
// recurses through all of them freely; that is exactly the shape #54's own
// `normalizeResult` had to hand-enumerate field by field, and exactly what
// this module now does once, for every model, forever.
//
// # Where a `null`/`undefined` distinction exists on the wire and is
// deliberately left alone
//
// `pageInfo.offset: number | null` appears in every paged envelope type in
// this package and is never read by any caller anywhere in this codebase
// (grepped before writing this comment) — normalizing it to `undefined` is
// harmless, and simpler than special-casing the one field out.
// `opt-outs.ts`'s `searchOptOutByMsisdn` result (`{ optOut: OptOutSummary |
// null }`) is exactly the same "absent nullable value" shape this module
// exists to fix, not an exception to it.
//
// # A second kind of value this walk must never touch: anything that isn't
// a plain object at all
//
// `typeof value === "object"` is also true for a `Date`, a `Map`, a `Set`,
// a `RegExp` — every exotic built-in, not just the plain `{}` objects
// `JSON.parse` produces. `Object.entries` on any of those silently returns
// `[]` rather than throwing, so without a guard this walk would rebuild a
// `Date` into a bare `{}`, discarding it with no error anywhere — the exact
// "a screen renders a confident wrong value instead of throwing" shape
// #221 itself is about, just one level removed from JSON nulls.
// `parseGatewayJson` alone could never hit this — `JSON.parse` never
// produces a `Date`/`Map`/`Set` — but `normalizeGatewayJson` is exported
// specifically for a value already in hand (see its own doc), so this
// function cannot assume its input came from `JSON.parse` just because its
// usual caller's input did. [`normalizeValue`]'s prototype check
// (`Object.getPrototypeOf(value) !== Object.prototype`) returns any such
// value completely untouched, the same "leave it alone rather than guess"
// discipline `VERBATIM_STRING_FIELDS` already applies by field name — this
// is the equivalent guard by *shape*, for values this module has no name
// to look up in advance. `json.test.ts` proves both that a `Date`/`Map`
// survive as the exact same instance and that a sibling `null` in the same
// object still normalizes.

/**
 * Schema columns that hold free-form or pre-serialised JSON, encoded as a
 * plain `String` (see this file's own module doc for why that already makes
 * them safe, and why this set exists anyway as a second, named guard).
 * Recursion stops at a matching key — the value, whatever it is, is
 * returned completely untouched.
 */
const VERBATIM_STRING_FIELDS: ReadonlySet<string> = new Set([
  // WebhookAttempt.payload — hooks.rs signs and sends these bytes exactly
  // as stored; never touch them even structurally.
  "payload",
  // cratestack_audit's own snapshot columns (audit_log.rs).
  "actor",
  "before",
  "after",
  "primaryKey",
  // Provider.config — a JSON-shaped `String` column (providers.ts's own doc).
  "config",
]);

function normalizeValue(value: unknown): unknown {
  if (value === null) return undefined;
  if (Array.isArray(value)) return value.map((entry) => normalizeValue(entry));
  if (typeof value === "object") {
    // A `Date`/`Map`/`Set`/`RegExp`/etc. is also `typeof "object"`, and
    // `Object.entries` on one of those silently returns `[]` rather than
    // throwing — without this guard, `normalizeValue` would rebuild any
    // such value into a bare `{}`, discarding it, with no error anywhere.
    // `JSON.parse` itself never produces one of these (the whole reason
    // `parseGatewayJson` was safe without this guard), but
    // `normalizeGatewayJson` is exported specifically for a value already
    // in hand — a test fixture, or a caller that didn't get here via
    // `JSON.parse` — so this function cannot assume its input is
    // JSON-shaped just because its usual caller's input is. A prototype
    // check (rather than an `instanceof` allowlist) covers every exotic
    // object shape, including ones nobody has written yet; `proto === null`
    // still falls through to the plain-object branch below, since
    // `Object.create(null)` is exactly what `JSON.parse` produces for an
    // object literal containing a `"__proto__"` key, and that value must
    // still be walked normally.
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) return value;

    const out: Record<string, unknown> = {};
    for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
      out[key] = VERBATIM_STRING_FIELDS.has(key) ? entry : normalizeValue(entry);
    }
    return out;
  }
  return value;
}

/**
 * The one place `null` -> `undefined` happens for every response this
 * package parses — see this file's own module doc. Exported alongside its
 * usual caller, {@link parseGatewayJson}, so a value already in hand (a
 * test fixture, or a body some future caller parses without going through
 * this module for some reason) can still go through the identical
 * normalization rather than a hand-rolled approximation of it.
 *
 * Because of that "value already in hand" use, this function's own input
 * isn't guaranteed to be `JSON.parse` output the way `parseGatewayJson`'s
 * always is — see [`normalizeValue`]'s own prototype guard (module doc's
 * "a second kind of value this walk must never touch" section) for why a
 * `Date`/`Map`/`Set`/etc. passed in here comes back as the exact same
 * value, not a silently emptied `{}`.
 */
export function normalizeGatewayJson<T>(value: T): T {
  return normalizeValue(value) as T;
}

interface JsonBodySource {
  text(): Promise<string>;
}

/**
 * Reads and parses an sms-api response body, applying
 * {@link normalizeGatewayJson} to the result. Replaces the `parseJsonBody`
 * every `@vsms/gateway` module used to carry its own byte-for-byte copy
 * of — consolidating it here is what makes the null-normalization above
 * genuinely singular, rather than "in every module that remembered to call
 * it," the exact duplicated-list smell #221 was filed over.
 *
 * An empty body (a `204`, or any response with no content) is `undefined`,
 * never attempted as JSON — matching every prior local copy. An unparseable
 * non-empty body is wrapped as a `{ code: "UNPARSEABLE_RESPONSE", message }`
 * object, also matching every prior local copy, so `mapGatewayError`
 * (`errors.ts`) sees the identical shape it always has.
 */
export async function parseGatewayJson(response: JsonBodySource): Promise<unknown> {
  const text = await response.text();
  if (text.length === 0) return undefined;
  try {
    return normalizeGatewayJson(JSON.parse(text));
  } catch {
    return { code: "UNPARSEABLE_RESPONSE", message: text };
  }
}
