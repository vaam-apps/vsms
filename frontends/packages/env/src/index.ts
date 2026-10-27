import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";
import { TUNING, withTuningCoerced } from "./tuning";

/** 32 bytes as unpadded base64url: 43 characters of the URL-safe alphabet. */
const KEY_32_BASE64URL = /^[A-Za-z0-9_-]{43}$/;

// What the console's server is configured with. There is no credential in this
// list on purpose: no client id, no signing key of the operator's, no session
// secret, no issuer. A person's rights come from a key their own browser made
// and a vaam admin phone vouched for (docs/architecture.md, "The console signs
// in the browser"), and the console's server only carries sealed bytes.
//
// What is here instead:
//   * where the gateway is, and how this process proves which *process* it is
//     (mTLS, when the URL is https);
//   * the gateway's two PUBLIC sealing keys, which the browser pins so it
//     believes only answers the gateway signed (`sms-gateway server-keys`
//     prints them; they are configuration, not secrets);
//   * the composer's service key, the one private key the server holds.
const created = createEnv({
  server: {
    SMS_API_URL: z.string().url(),
    SMS_API_CLIENT_CERT_PATH: z.string().optional(),
    SMS_API_CLIENT_KEY_PATH: z.string().optional(),
    SMS_API_CA_PATH: z.string().optional(),
    // The gateway's response-signing keys, `current` and `next`, as
    // `sms-gateway server-keys` prints them: 32 bytes of unpadded base64url
    // each, and different (with one pinned a rotation is a flag day). The
    // layout hands them to the browser, which pins them.
    SMS_ENVELOPE_SERVER_KEY_CURRENT: z.string().regex(KEY_32_BASE64URL),
    SMS_ENVELOPE_SERVER_KEY_NEXT: z.string().regex(KEY_32_BASE64URL),
    // The composer's own service key: an Ed25519 PKCS#8 PEM file, registered
    // with `sms-gateway register-client --client-name console-composer`
    // (docs/runbooks/deployment.adoc, "The composer's key"). Read when the
    // composer first sends, so a console deployed without one still serves
    // every other screen.
    SMS_COMPOSER_KEY_FILE: z.string().min(1).optional(),
    MESSAGE_STREAM_POLL_MS: TUNING.MESSAGE_STREAM_POLL_MS,
    // R6 (AGENTS.md): a tuning value, same shape as `MESSAGE_STREAM_POLL_MS`
    // above — one place, validated at boot, instead of a `REFETCH_INTERVAL_MS
    // = 5000` const copy-pasted per screen. This is the Jobs (#56), Workers
    // (#57), and Webhooks (#55) screens' shared diagnostics-poll cadence —
    // the R6 sweep landed the Webhooks screen with its own second env var
    // carrying this identical 5000ms value under a different name; the two
    // were reconciled onto this one entry (R6-reconcile) per this comment's
    // own original anticipation of exactly that convergence.
    // `dashboard-screen.tsx`'s 15_000ms is a materially different cadence
    // (an overview, not a diagnostics table) and deliberately gets no entry
    // here — it needs its own, sized for what it actually polls.
    DIAGNOSTICS_POLL_MS: TUNING.DIAGNOSTICS_POLL_MS,
    // R6 (Administration group, `audit-log-screen.tsx`): a page size is an
    // operational tuning value, not a protocol constant — the same test
    // `MESSAGE_STREAM_POLL_MS` already sets a precedent for. Read
    // server-side in `audit-log/page.tsx` and handed down as a prop, the
    // identical shape that file already uses.
    AUDIT_LOG_PAGE_SIZE: TUNING.AUDIT_LOG_PAGE_SIZE,
    LIST_PAGE_SIZE: TUNING.LIST_PAGE_SIZE,
    // Dashboard screen (#49). Same "operational tuning value, not
    // protocol/security" reasoning AGENTS.md's R6 already gives for
    // `MESSAGE_STREAM_POLL_MS` — a hoisted `REFETCH_INTERVAL_MS` in the
    // screen file itself was the R6 violation; this is the fix, not a
    // shared constant, since the dashboard's own 15_000ms overview cadence
    // is materially different from `DIAGNOSTICS_POLL_MS`'s diagnostics-table
    // cadence above, not merely a fourth independent copy of the same value.
    DASHBOARD_REFETCH_INTERVAL_MS: TUNING.DASHBOARD_REFETCH_INTERVAL_MS,
    QUERY_STALE_TIME_MS: TUNING.QUERY_STALE_TIME_MS,
    NODE_ENV: z.enum(["development", "production", "test"]).default("development"),
  },
  client: {
    NEXT_PUBLIC_APP_NAME: z.string(),
  },
  runtimeEnv: {
    // Server
    SMS_API_URL: process.env.SMS_API_URL,
    SMS_API_CLIENT_CERT_PATH: process.env.SMS_API_CLIENT_CERT_PATH,
    SMS_API_CLIENT_KEY_PATH: process.env.SMS_API_CLIENT_KEY_PATH,
    SMS_API_CA_PATH: process.env.SMS_API_CA_PATH,
    SMS_ENVELOPE_SERVER_KEY_CURRENT: process.env.SMS_ENVELOPE_SERVER_KEY_CURRENT,
    SMS_ENVELOPE_SERVER_KEY_NEXT: process.env.SMS_ENVELOPE_SERVER_KEY_NEXT,
    SMS_COMPOSER_KEY_FILE: process.env.SMS_COMPOSER_KEY_FILE,
    MESSAGE_STREAM_POLL_MS: process.env.MESSAGE_STREAM_POLL_MS,
    DIAGNOSTICS_POLL_MS: process.env.DIAGNOSTICS_POLL_MS,
    AUDIT_LOG_PAGE_SIZE: process.env.AUDIT_LOG_PAGE_SIZE,
    LIST_PAGE_SIZE: process.env.LIST_PAGE_SIZE,
    DASHBOARD_REFETCH_INTERVAL_MS: process.env.DASHBOARD_REFETCH_INTERVAL_MS,
    QUERY_STALE_TIME_MS: process.env.QUERY_STALE_TIME_MS,
    NODE_ENV: process.env.NODE_ENV,
    // Client
    NEXT_PUBLIC_APP_NAME: process.env.NEXT_PUBLIC_APP_NAME,
  },
  skipValidation: !!process.env.SKIP_ENV_VALIDATION,
});

/**
 * `SKIP_ENV_VALIDATION` skips the whole schema, coercion included, so
 * without this every tuning number reaches its caller as the raw string from
 * `process.env` (or `undefined`). Both demo compose files run the console
 * with it set. Measured there: `"2000"` made the messages long-poll's
 * deadline a string concatenation, every poll returned in ~30ms instead of
 * waiting 2s, and the Messages screen sent ~12 requests a second; and
 * TanStack Query ignores a string `refetchInterval` outright, so the
 * dashboard, jobs, workers and webhook-attempt polls never ran at all.
 * The tuning values are coerced (or defaulted) even when validation is
 * skipped; everything else is untouched.
 */
export const env = process.env.SKIP_ENV_VALIDATION ? withTuningCoerced(created) : created;

// Cross-field validation rules
// The cross-field rules below read `env` values directly. When
// SKIP_ENV_VALIDATION is set (CI builds, where no real auth config or TLS
// upstream exists), those values are unvalidated and may be undefined — so
// running these checks would throw `TypeError: Invalid URL` rather than
// skipping, which defeats the escape hatch entirely. Skip them together.
const skipValidation = !!process.env.SKIP_ENV_VALIDATION;

if (!skipValidation) {
  // `next build` sets NODE_ENV=production while *compiling*, which is not the
  // same thing as *running* a production server. Next signals the compile with
  // NEXT_PHASE=phase-production-build. The rule below exists to stop a
  // production deployment talking to its gateway over plaintext: a deploy-time
  // guarantee, not a compile-time one, so applying it during a build makes the
  // workspace unbuildable on a developer machine and in CI (where no TLS
  // upstream exists) while protecting nothing. Runtime boot is still fully
  // checked: this only exempts the build.
  const isProductionBuild = process.env.NEXT_PHASE === "phase-production-build";
  const enforceDeploymentRules = env.NODE_ENV === "production" && !isProductionBuild;

  if (env.SMS_ENVELOPE_SERVER_KEY_CURRENT === env.SMS_ENVELOPE_SERVER_KEY_NEXT) {
    throw new Error(
      "SMS_ENVELOPE_SERVER_KEY_CURRENT and SMS_ENVELOPE_SERVER_KEY_NEXT are the same key: pin " +
        "both of the gateway's keys (`sms-gateway server-keys`); with one, a rotation is a flag day",
    );
  }

  const apiUrl = new URL(env.SMS_API_URL);
  const isHttps = apiUrl.protocol === "https:";

  if (isHttps) {
    if (!env.SMS_API_CLIENT_CERT_PATH || !env.SMS_API_CLIENT_KEY_PATH || !env.SMS_API_CA_PATH) {
      throw new Error(
        "SMS_API_URL uses https: protocol, so all three cert paths must be set: SMS_API_CLIENT_CERT_PATH, SMS_API_CLIENT_KEY_PATH, SMS_API_CA_PATH",
      );
    }
  } else {
    if (env.SMS_API_CLIENT_CERT_PATH || env.SMS_API_CLIENT_KEY_PATH || env.SMS_API_CA_PATH) {
      throw new Error(
        "SMS_API_URL uses http: protocol, so cert paths must NOT be set: SMS_API_CLIENT_CERT_PATH, SMS_API_CLIENT_KEY_PATH, SMS_API_CA_PATH",
      );
    }
  }

  if (enforceDeploymentRules && !isHttps) {
    throw new Error("NODE_ENV=production requires SMS_API_URL to use https: protocol");
  }
}
