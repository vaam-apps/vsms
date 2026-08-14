// #211's own core guarantee, tested directly: `resolveUpstreamAccessToken`
// must resolve to the ambient credential's own token, must throw rather
// than default to the machine credential when no scope was ever entered,
// and — the property that makes it safe against `MessageStreamHub`'s
// process-wide `setInterval` poll (see `messages.ts`'s own doc on
// `getJsonWith`) — a credential set for one `runWithRequestCredential` call
// must never leak into a concurrent, unrelated one.

import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./token", () => ({
  getMachineAccessToken: vi.fn().mockResolvedValue("machine-token"),
  invalidateMachineAccessToken: vi.fn(),
}));

import {
  invalidateUpstreamAccessToken,
  resolveUpstreamAccessToken,
  runWithRequestCredential,
} from "./request-credential";
import { getMachineAccessToken, invalidateMachineAccessToken } from "./token";

afterEach(() => {
  vi.clearAllMocks();
});

describe("resolveUpstreamAccessToken", () => {
  it("throws when called outside any runWithRequestCredential scope — the whole point: a new call site that forgets to wrap itself must fail loudly, never silently fall back to the machine credential", async () => {
    await expect(resolveUpstreamAccessToken()).rejects.toThrow(
      /outside a request-credential scope/,
    );
  });

  it("resolves to the human credential's own accessToken, verbatim, without touching the machine token cache", async () => {
    const token = await runWithRequestCredential({ kind: "human", accessToken: "human-jwt" }, () =>
      resolveUpstreamAccessToken(),
    );
    expect(token).toBe("human-jwt");
    expect(getMachineAccessToken).not.toHaveBeenCalled();
  });

  it("delegates to getMachineAccessToken for a machine-credential scope", async () => {
    const token = await runWithRequestCredential({ kind: "machine" }, () =>
      resolveUpstreamAccessToken(),
    );
    expect(token).toBe("machine-token");
    expect(getMachineAccessToken).toHaveBeenCalledTimes(1);
  });

  it("propagates the ambient credential across an await, matching how a real fetch call site uses it", async () => {
    await runWithRequestCredential({ kind: "human", accessToken: "human-jwt" }, async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(await resolveUpstreamAccessToken()).toBe("human-jwt");
    });
  });

  it("never leaks one call's credential into a concurrent, unrelated call — the exact hazard a shared setInterval-driven poller (message-stream.ts) would otherwise hit", async () => {
    const results = await Promise.all([
      runWithRequestCredential({ kind: "human", accessToken: "alice-token" }, async () => {
        await new Promise((resolve) => setTimeout(resolve, 5));
        return resolveUpstreamAccessToken();
      }),
      runWithRequestCredential({ kind: "human", accessToken: "bob-token" }, async () => {
        return resolveUpstreamAccessToken();
      }),
    ]);
    expect(results).toEqual(["alice-token", "bob-token"]);
  });
});

describe("invalidateUpstreamAccessToken", () => {
  it("invalidates the machine token cache for a machine-credential scope", async () => {
    await runWithRequestCredential({ kind: "machine" }, async () => {
      invalidateUpstreamAccessToken();
    });
    expect(invalidateMachineAccessToken).toHaveBeenCalledTimes(1);
  });

  it("is a no-op for a human-credential scope — there is no per-process cache to invalidate; see this module's own doc on why a same-request retry with the identical token is the correct, if wasteful, behaviour", async () => {
    await runWithRequestCredential({ kind: "human", accessToken: "human-jwt" }, async () => {
      invalidateUpstreamAccessToken();
    });
    expect(invalidateMachineAccessToken).not.toHaveBeenCalled();
  });

  it("invalidates the machine cache when called outside any scope — the conservative default for a defensive/never-expected call path", () => {
    invalidateUpstreamAccessToken();
    expect(invalidateMachineAccessToken).toHaveBeenCalledTimes(1);
  });
});
