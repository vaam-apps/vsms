import { createEnv } from "@t3-oss/env-nextjs";
import { z } from "zod";

export const env = createEnv({
  server: {
    DASHBOARD_AUTH: z.enum(["none", "basic"]).default("none"),
    DASHBOARD_BASIC_REALM: z.string().optional(),
    DASHBOARD_BASIC_USERS: z.string().optional(),
    SMS_API_URL: z.string().url(),
    SMS_API_CLIENT_CERT_PATH: z.string().optional(),
    SMS_API_CLIENT_KEY_PATH: z.string().optional(),
    SMS_API_CA_PATH: z.string().optional(),
    SMS_AUTH_ISSUER: z.string().url(),
    SMS_CONSOLE_CLIENT_ID: z.string(),
    SMS_CONSOLE_PRIVATE_KEY_PATH: z.string(),
    SMS_CONSOLE_SCOPE: z.string().min(1).default("sms:send sms:read"),
    MESSAGE_STREAM_POLL_MS: z.coerce.number().int().min(500).default(2000),
    NODE_ENV: z.enum(["development", "production", "test"]).default("development"),
  },
  client: {
    NEXT_PUBLIC_APP_NAME: z.string(),
  },
  runtimeEnv: {
    // Server
    DASHBOARD_AUTH: process.env.DASHBOARD_AUTH,
    DASHBOARD_BASIC_REALM: process.env.DASHBOARD_BASIC_REALM,
    DASHBOARD_BASIC_USERS: process.env.DASHBOARD_BASIC_USERS,
    SMS_API_URL: process.env.SMS_API_URL,
    SMS_API_CLIENT_CERT_PATH: process.env.SMS_API_CLIENT_CERT_PATH,
    SMS_API_CLIENT_KEY_PATH: process.env.SMS_API_CLIENT_KEY_PATH,
    SMS_API_CA_PATH: process.env.SMS_API_CA_PATH,
    SMS_AUTH_ISSUER: process.env.SMS_AUTH_ISSUER,
    SMS_CONSOLE_CLIENT_ID: process.env.SMS_CONSOLE_CLIENT_ID,
    SMS_CONSOLE_PRIVATE_KEY_PATH: process.env.SMS_CONSOLE_PRIVATE_KEY_PATH,
    SMS_CONSOLE_SCOPE: process.env.SMS_CONSOLE_SCOPE,
    MESSAGE_STREAM_POLL_MS: process.env.MESSAGE_STREAM_POLL_MS,
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
  if (env.DASHBOARD_AUTH === "basic") {
    if (!env.DASHBOARD_BASIC_USERS || env.DASHBOARD_BASIC_USERS.trim() === "") {
      throw new Error(
        'DASHBOARD_AUTH=basic requires DASHBOARD_BASIC_USERS to be non-empty (format: "username:sha256hex,...")',
      );
    }

    const userPattern = /^[^:,]+:[0-9a-f]{64}$/;
    const users = env.DASHBOARD_BASIC_USERS.split(",").map((u) => u.trim());
    for (const user of users) {
      if (!userPattern.test(user)) {
        throw new Error(
          `DASHBOARD_BASIC_USERS entry '${user}' does not match expected format "username:sha256hex" (64 hex chars)`,
        );
      }
    }
  }

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

  if (enforceDeploymentRules && env.DASHBOARD_AUTH === "none") {
    throw new Error(
      "NODE_ENV=production requires DASHBOARD_AUTH to be set (basic or other auth method)",
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
