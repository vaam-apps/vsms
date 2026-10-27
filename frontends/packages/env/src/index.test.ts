// The console's boot-time validation, as a module import: `@vsms/env` throws
// when it is loaded with a configuration the console cannot run on.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const CURRENT = "A".repeat(43);
const NEXT = "B".repeat(43);

const SAVED = { ...process.env };

beforeEach(() => {
  vi.resetModules();
  for (const key of Object.keys(process.env)) {
    if (key.startsWith("SMS_") || key === "SKIP_ENV_VALIDATION") delete process.env[key];
  }
  process.env.NEXT_PUBLIC_APP_NAME = "vsms test";
  process.env.SMS_ENVELOPE_SERVER_KEY_CURRENT = CURRENT;
  process.env.SMS_ENVELOPE_SERVER_KEY_NEXT = NEXT;
});

afterEach(() => {
  for (const key of Object.keys(process.env)) delete process.env[key];
  Object.assign(process.env, SAVED);
});

describe("SMS_API_URL", () => {
  it("boots on the gateway's origin", async () => {
    process.env.SMS_API_URL = "http://sms-gateway:8080";

    const { env } = await import("./index");

    expect(env.SMS_API_URL).toBe("http://sms-gateway:8080");
  });

  it("does not boot on a URL with a path prefix, which every request path would replace", async () => {
    process.env.SMS_API_URL = "http://edge.example.com/vsms";

    await expect(import("./index")).rejects.toThrow(/no path/);
  });
});
