import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";

export const env = createEnv({
  server: {
    SMS_API_URL: z.string().url(),
    SMS_API_CLIENT_CERT_PATH: z.string().optional(),
    SMS_API_CLIENT_KEY_PATH: z.string().optional(),
    SMS_API_CA_PATH: z.string().optional(),
    SMS_AUTH_ISSUER: z.string().url(),
    SMS_CONSOLE_CLIENT_ID: z.string(),
    SMS_CONSOLE_PRIVATE_KEY_PATH: z.string(),
    // #56/#57: job:read/job:enqueue (Jobs screen) and worker:read (Workers
    // screen) — the console's own AppClient must be *provisioned* with
    // these scopes too (`scripts/demo.sh`'s `provision-client` call, or the
    // getting-started runbook's own `--scope` flags); requesting a scope
    // the client isn't registered for at `/token` fails the exchange, and
    // omitting it here even when the client *is* registered still denies
    // every call at Layer 2 (`sms_api::rbac::require_permission` — an
    // absent scope claim is denial, never a fallback to "whatever the
    // client is allowed to ask for").
    SMS_CONSOLE_SCOPE: z
      .string()
      .min(1)
      .default("sms:send sms:read job:read job:enqueue worker:read"),
    // #194: the human authorization-code + PKCE flow. DASHBOARD_AUTH
    // (none|basic) is gone — a hard cutover, not a parallel path: see
    // frontends/apps/admin/middleware.ts's own module doc for why leaving Basic auth
    // reachable behind a flag was rejected. ADMIN_BASE_URL is this
    // console's own externally-reachable origin — the one fixed
    // `redirect_uri` (`${ADMIN_BASE_URL}/api/auth/callback`) the
    // `sms-console` OauthClient row is registered with; an exact-match
    // requirement per RFC 6749 §3.1.2, so this must be the literal origin
    // browsers reach this app at, not an internal/loopback address.
    ADMIN_BASE_URL: z.string().url(),
    SMS_CONSOLE_OIDC_CLIENT_ID: z.string().min(1).default("sms-console"),
    // AES-256-GCM needs 32 raw bytes once hashed down (frontends/apps/admin/lib/oidc.ts) —
    // require real entropy up front rather than accepting a short string
    // that would still "work" (any string hashes to 32 bytes) but with far
    // less real keyspace than the cookie's own encryption implies.
    SMS_CONSOLE_SESSION_SECRET: z.string().min(32),
    MESSAGE_STREAM_POLL_MS: z.coerce.number().int().min(500).default(2000),
    // Dashboard screen (#49). Same "operational tuning value, not
    // protocol/security" reasoning AGENTS.md's R6 already gives for
    // `MESSAGE_STREAM_POLL_MS` — a hoisted `REFETCH_INTERVAL_MS` in the
    // screen file itself was the R6 violation; this is the fix, not a
    // shared constant, since `jobs-screen.tsx`/`workers-screen.tsx`/
    // `webhooks-screen.tsx` each own their own independent 5000ms copy of
    // the same *kind* of decision, not this same value — merging the four
    // into one env var is a separate call for whoever owns those screens.
    DASHBOARD_REFETCH_INTERVAL_MS: z.coerce.number().int().min(1000).default(15_000),
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
    SMS_AUTH_ISSUER: process.env.SMS_AUTH_ISSUER,
    SMS_CONSOLE_CLIENT_ID: process.env.SMS_CONSOLE_CLIENT_ID,
    SMS_CONSOLE_PRIVATE_KEY_PATH: process.env.SMS_CONSOLE_PRIVATE_KEY_PATH,
    SMS_CONSOLE_SCOPE: process.env.SMS_CONSOLE_SCOPE,
    ADMIN_BASE_URL: process.env.ADMIN_BASE_URL,
    SMS_CONSOLE_OIDC_CLIENT_ID: process.env.SMS_CONSOLE_OIDC_CLIENT_ID,
    SMS_CONSOLE_SESSION_SECRET: process.env.SMS_CONSOLE_SESSION_SECRET,
    MESSAGE_STREAM_POLL_MS: process.env.MESSAGE_STREAM_POLL_MS,
    DASHBOARD_REFETCH_INTERVAL_MS: process.env.DASHBOARD_REFETCH_INTERVAL_MS,
    NODE_ENV: process.env.NODE_ENV,
    // Client
    NEXT_PUBLIC_APP_NAME: process.env.NEXT_PUBLIC_APP_NAME,
  },
  skipValidation: !!process.env.SKIP_ENV_VALIDATION,
});

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
  // NEXT_PHASE=phase-production-build. The two rules below exist to stop a
  // production deployment being served open to the internet or over plaintext —
  // they are deploy-time guarantees, not compile-time ones — so applying them
  // during a build makes the workspace unbuildable on a developer machine and in
  // CI (where no real auth config or TLS upstream exists) while protecting
  // nothing. Runtime boot is still fully checked: this only exempts the build.
  const isProductionBuild = process.env.NEXT_PHASE === "phase-production-build";
  const enforceDeploymentRules = env.NODE_ENV === "production" && !isProductionBuild;

  // #194's own hard requirement: session cookies must be Secure, which
  // means an HTTPS ADMIN_BASE_URL in production — a `redirect_uri` (and a
  // cookie) served over plaintext defeats the point of encrypting the
  // session in the first place.
  if (enforceDeploymentRules && new URL(env.ADMIN_BASE_URL).protocol !== "https:") {
    throw new Error("NODE_ENV=production requires ADMIN_BASE_URL to use https: protocol");
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
