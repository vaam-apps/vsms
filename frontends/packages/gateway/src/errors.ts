import "server-only";

// Mapping sms-api's `CoolErrorResponse` (`{ code, message, details }` —
// see `cratestack-core`'s `error.rs`) onto the vocabulary tRPC's error
// formatter understands, so a `TRPCError` built from a `GatewayError`'s
// `trpcCode` carries the right HTTP status all the way back to the
// browser.
//
// The status-to-code table is exactly what the task specifies:
//
//   422 (VALIDATION_ERROR)      -> BAD_REQUEST, `fieldErrors` preserved
//                                  for react-hook-form's `setError`
//   409 (CONFLICT / SM001)      -> CONFLICT
//   412 (PreconditionFailed)    -> CONFLICT
//   401 / 403                   -> FORBIDDEN, plus a server-side log —
//                                  a caller with a valid session should
//                                  never see the machine token get
//                                  rejected; that is an operator problem,
//                                  not a user one, so it is logged loudly
//                                  here rather than only surfaced as a
//                                  generic "forbidden" toast.
//   everything else             -> INTERNAL_SERVER_ERROR
//
// AGENTS.md records the same policy-gap shape being found live *seven*
// separate times: a model whose `@@allow` forgot `hasRole('system')`
// doesn't error, it silently returns an empty array, and the calling code
// behaves as though the table were empty. `compose.preview`/`compose.send`
// are procedures, not list routes, so that specific failure mode can't
// reach this module yet — there is no list to come back empty. The hook
// belongs here in principle (log `warn` on any zero-length list from an
// admin model) but is deliberately not implemented until T11 adds the
// first admin list procedure this client actually calls: writing it now
// against nothing would be exactly the kind of dormant, never-exercised
// code this codebase's own conventions reject. Whoever adds the first
// list call should read this comment and add the check then.

/** tRPC's own `TRPC_ERROR_CODE_KEY` vocabulary — not imported from
 * `@trpc/server` here so `@vsms/gateway` stays decoupled from the
 * specific RPC transport (`@vsms/api` is the seam that owns the tRPC
 * dependency); the string values are identical to tRPC's own codes by
 * construction, so `frontends/packages/api/src/routers/compose.ts` can pass
 * `error.trpcCode` straight into `new TRPCError({ code, ... })`. */
export type GatewayTrpcCode = "BAD_REQUEST" | "CONFLICT" | "FORBIDDEN" | "INTERNAL_SERVER_ERROR";

export interface GatewayFieldErrors {
  [field: string]: string[];
}

/** The shape `cratestack-core::CoolErrorResponse` serialises to. */
interface CoolErrorResponse {
  code: string;
  message: string;
  details?: unknown;
}

/**
 * Thrown by every `@vsms/gateway` call that reaches sms-api and gets back
 * a non-2xx `CoolErrorResponse`-shaped body (or a response this module
 * can't parse as one, in which case `code`/`message` describe the parse
 * failure instead and `httpStatus`/`trpcCode` still reflect the real HTTP
 * status). `frontends/packages/api/src/routers/compose.ts` catches this and
 * re-throws as a `TRPCError`.
 */
export class GatewayError extends Error {
  readonly httpStatus: number;
  readonly trpcCode: GatewayTrpcCode;
  readonly gatewayCode: string | undefined;
  readonly fieldErrors: GatewayFieldErrors | undefined;

  constructor(
    message: string,
    options: {
      httpStatus: number;
      trpcCode: GatewayTrpcCode;
      gatewayCode?: string;
      fieldErrors?: GatewayFieldErrors;
    },
  ) {
    super(message);
    this.name = "GatewayError";
    this.httpStatus = options.httpStatus;
    this.trpcCode = options.trpcCode;
    this.gatewayCode = options.gatewayCode;
    this.fieldErrors = options.fieldErrors;
  }
}

function isCoolErrorResponse(body: unknown): body is CoolErrorResponse {
  return (
    typeof body === "object" &&
    body !== null &&
    "code" in body &&
    "message" in body &&
    typeof (body as { code: unknown }).code === "string" &&
    typeof (body as { message: unknown }).message === "string"
  );
}

/**
 * Best-effort extraction of per-field errors from `CoolErrorResponse.details`.
 * The framework version this repo pins (`cratestack-pg =0.5.0`) does not
 * yet populate `details` on `Validation` errors — see `cratestack-core`'s
 * `error.rs`, every construction site sets `details: None` — so this
 * almost always returns `undefined` today. It's still worth doing
 * defensively rather than assuming the shape: a future framework version
 * populating `details` with a `{ field: [messages] }`-shaped `Value`
 * should flow through automatically, and anything shaped differently is
 * safely ignored rather than crashing the error path itself.
 */
function extractFieldErrors(details: unknown): GatewayFieldErrors | undefined {
  if (typeof details !== "object" || details === null) return undefined;
  const out: GatewayFieldErrors = {};
  let sawAny = false;
  for (const [field, value] of Object.entries(details as Record<string, unknown>)) {
    if (Array.isArray(value) && value.every((v) => typeof v === "string")) {
      out[field] = value;
      sawAny = true;
    }
  }
  return sawAny ? out : undefined;
}

function trpcCodeForStatus(status: number): GatewayTrpcCode {
  switch (status) {
    case 422:
      return "BAD_REQUEST";
    case 409:
    case 412:
      return "CONFLICT";
    case 401:
    case 403:
      return "FORBIDDEN";
    default:
      return "INTERNAL_SERVER_ERROR";
  }
}

/**
 * Builds a {@link GatewayError} from an sms-api HTTP response's status and
 * parsed (or unparseable) JSON body. `procedure` is only for the log
 * line's benefit — it names which `$procs/*` call failed.
 */
export function mapGatewayError(status: number, body: unknown, procedure: string): GatewayError {
  const trpcCode = trpcCodeForStatus(status);

  if (status === 401 || status === 403) {
    // #211 correction: this used to assert unconditionally that a 401/403
    // here always means the console's own machine credential was rejected
    // (a scope mismatch, a retired client, a signing-key rotation) —
    // written before `resolveUpstreamAccessToken` existed, when the
    // machine credential really was the only thing ever presented
    // upstream. That's no longer true: most calls now forward the
    // signed-in human's own session token, and a 403 from those is
    // frequently an entirely ordinary, expected outcome — a `support`
    // account clicking an action their role's own permissions don't cover,
    // for instance (#58's opt-out screens are exactly this shape). This
    // module has no visibility into which credential a given call used
    // (that decision lives in `request-credential.ts`, one layer down), so
    // it can no longer claim to know which one failed — logged as
    // information for whoever operates this deployment, not asserted as a
    // fault.
    console.error(
      `[@vsms/gateway] ${procedure}: sms-api returned ${status}. Could be an ordinary ` +
        `permission denial for the signed-in caller, or the console's own machine credential ` +
        `(SMS_CONSOLE_CLIENT_ID) being rejected — check which credential this call used before ` +
        `assuming either.`,
      body,
    );
  }

  if (!isCoolErrorResponse(body)) {
    return new GatewayError(`sms-api ${procedure} failed with status ${status}`, {
      httpStatus: status,
      trpcCode,
    });
  }

  const fieldErrors = status === 422 ? extractFieldErrors(body.details) : undefined;
  return new GatewayError(body.message, {
    httpStatus: status,
    trpcCode,
    gatewayCode: body.code,
    // exactOptionalPropertyTypes: only assign the key when there's a real
    // value — assigning `undefined` explicitly is a type error against an
    // optional (not `T | undefined`) property.
    ...(fieldErrors !== undefined ? { fieldErrors } : {}),
  });
}

/**
 * #59: the one thing an edit screen actually needs to branch on — "someone
 * else changed this row since I loaded it, reload and try again" — rather
 * than the generic `trpcCode: "CONFLICT"` bucket a duplicate-key 409 also
 * falls into. Checks `httpStatus` (always present, even when the response
 * body didn't parse as a `CoolErrorResponse`) rather than `gatewayCode`
 * alone, so a stale `If-Match` is still recognised as such even against a
 * malformed error body — matching this module's own "a vague 409 beats a
 * misleading 500" bias toward a still-actionable answer over a precise one
 * that only works on the happy path.
 */
export function isStaleWriteError(error: unknown): error is GatewayError {
  return error instanceof GatewayError && error.httpStatus === 412;
}
