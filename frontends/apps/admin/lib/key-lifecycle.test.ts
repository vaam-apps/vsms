// Where the key stands: the three sources joined (IndexedDB, the gateway, the
// clock), and the one thing only the browser can add to the gateway's answer,
// that a key which was accepted once and is refused now is a key that ENDED.

import { describe, expect, it, vi } from "vitest";
import type { PairedMeta, StoredKey } from "./device-key";
import {
  daysLeft,
  isExpired,
  readKeyStatus,
  type StatusDeps,
  shouldRereadOnRefusal,
} from "./key-lifecycle";

const NOW = Date.parse("2026-10-27T12:00:00Z");
const DAY = 86_400_000;

function meta(expiresInDays: number): PairedMeta {
  return {
    keyId: "ck1",
    userId: "u1",
    pairedAt: new Date(NOW - (30 - expiresInDays) * DAY).toISOString(),
    expiresAt: new Date(NOW + expiresInDays * DAY).toISOString(),
  };
}

const stored = (paired?: PairedMeta): StoredKey => ({
  pair: {} as CryptoKeyPair,
  createdAt: NOW - DAY,
  paired,
});

const refusal = (httpStatus: number) => Object.assign(new Error("refused"), { httpStatus });

function deps(over: Partial<StatusDeps> = {}): StatusDeps {
  return {
    load: async () => stored(),
    probe: async () => undefined,
    lookup: async () => ({
      id: "ck1",
      userId: "u1",
      pairedAt: new Date(NOW).toISOString(),
      expiresAt: new Date(NOW + 30 * DAY).toISOString(),
    }),
    thumbprint: async () => "ab".repeat(32),
    remember: async () => undefined,
    now: () => NOW,
    skew: async () => 0,
    ...over,
  };
}

describe("readKeyStatus", () => {
  it("no key in this browser: none, and the gateway is not asked", async () => {
    const probe = vi.fn();
    expect(await readKeyStatus(deps({ load: async () => undefined, probe }))).toEqual({
      kind: "none",
    });
    expect(probe).not.toHaveBeenCalled();
  });

  it("a key the gateway does not know yet, never accepted: pending, not ended", async () => {
    const status = await readKeyStatus(
      deps({
        probe: async () => {
          throw refusal(401);
        },
      }),
    );

    expect(status).toEqual({ kind: "pending" });
  });

  it("the gateway accepts a new key: paired, with the id and expiry the gateway's row names, remembered", async () => {
    const remember = vi.fn(async () => undefined);

    const status = await readKeyStatus(deps({ remember }));

    expect(status).toMatchObject({ kind: "paired", meta: { keyId: "ck1", userId: "u1" } });
    expect(remember).toHaveBeenCalledWith(expect.objectContaining({ keyId: "ck1" }));
    const asked = (status as { meta: PairedMeta }).meta;
    expect(daysLeft(asked, NOW)).toBe(30);
  });

  it("a key that was accepted and now is refused has ENDED: expired or revoked, never 'pending'", async () => {
    const status = await readKeyStatus(
      deps({
        load: async () => stored(meta(10)),
        probe: async () => {
          throw refusal(401);
        },
      }),
    );

    expect(status).toMatchObject({ kind: "ended", cause: "refused" });
  });

  it("a refusal on a computer whose clock is off is the clock's, not the key's: the key is left alone", async () => {
    const refused = async () => {
      throw refusal(401);
    };

    for (const skew of [301, -301, 600]) {
      const status = await readKeyStatus(
        deps({ load: async () => stored(meta(10)), probe: refused, skew: async () => skew }),
      );
      expect(status, `${skew}s`).toMatchObject({ kind: "clock", skewSeconds: skew });
      expect((status as { meta: PairedMeta }).meta.keyId).toBe("ck1");
    }
    // Within the warning threshold it is the key.
    expect(
      await readKeyStatus(
        deps({ load: async () => stored(meta(10)), probe: refused, skew: async () => 60 }),
      ),
    ).toMatchObject({ kind: "ended", cause: "refused" });
    // A skew that could not be measured is not evidence of one.
    expect(
      await readKeyStatus(
        deps({ load: async () => stored(meta(10)), probe: refused, skew: async () => undefined }),
      ),
    ).toMatchObject({ kind: "ended", cause: "refused" });
  });

  it("a key that looks expired on a wrong clock is not called expired", async () => {
    const probe = vi.fn();

    const status = await readKeyStatus(
      deps({ load: async () => stored(meta(-1)), probe, skew: async () => 3600 }),
    );

    expect(status).toMatchObject({ kind: "clock" });
    expect(probe).not.toHaveBeenCalled();
  });

  it("a skew that cannot be read never turns a read into a throw", async () => {
    const status = await readKeyStatus(
      deps({
        load: async () => stored(meta(10)),
        probe: async () => {
          throw refusal(401);
        },
        skew: async () => {
          throw new Error("offline");
        },
      }),
    );

    expect(status).toMatchObject({ kind: "ended", cause: "refused" });
  });

  it("a key past its 30 days has ended on the clock alone, without asking the gateway", async () => {
    const probe = vi.fn();

    const status = await readKeyStatus(deps({ load: async () => stored(meta(-1)), probe }));

    expect(status).toMatchObject({ kind: "ended", cause: "expired" });
    expect(probe).not.toHaveBeenCalled();
  });

  it("a gateway that cannot be reached is unreachable, never 'not paired'", async () => {
    for (const failure of [new Error("fetch failed"), refusal(503), refusal(502)]) {
      const status = await readKeyStatus(
        deps({
          load: async () => stored(meta(10)),
          probe: async () => {
            throw failure;
          },
        }),
      );
      expect(status).toEqual({ kind: "unreachable" });
    }
  });

  it("a paired key stays paired without another lookup", async () => {
    const lookup = vi.fn();

    const status = await readKeyStatus(deps({ load: async () => stored(meta(10)), lookup }));

    expect(status).toMatchObject({ kind: "paired" });
    expect(lookup).not.toHaveBeenCalled();
  });

  it("an IndexedDB that cannot be opened is a browser with no key", async () => {
    const status = await readKeyStatus(
      deps({
        load: async () => {
          throw new Error("SecurityError");
        },
      }),
    );

    expect(status).toEqual({ kind: "none" });
  });

  it("an accepted key whose row cannot be read still counts as paired, and is not remembered", async () => {
    const remember = vi.fn();

    const status = await readKeyStatus(
      deps({
        lookup: async () => {
          throw refusal(500);
        },
        remember,
      }),
    );

    expect(status).toMatchObject({ kind: "paired", meta: { keyId: "" } });
    expect(remember).not.toHaveBeenCalled();
  });
});

describe("expiry", () => {
  it("counts whole days, rounded up, and never below zero", () => {
    expect(daysLeft(meta(29.2), NOW)).toBe(30);
    expect(daysLeft(meta(1), NOW)).toBe(1);
    expect(daysLeft(meta(0.01), NOW)).toBe(1);
    expect(daysLeft(meta(-3), NOW)).toBe(0);
  });

  it("is expired at the instant it expires", () => {
    expect(isExpired({ ...meta(1), expiresAt: new Date(NOW).toISOString() }, NOW)).toBe(true);
    expect(isExpired(meta(1), NOW)).toBe(false);
  });
});

describe("shouldRereadOnRefusal", () => {
  it("reads again only for a paired console whose key was just refused, and not while a read is in flight", () => {
    expect(shouldRereadOnRefusal("paired", false)).toBe(true);
    expect(shouldRereadOnRefusal("paired", true)).toBe(false);
  });

  it("never reads again for any other status: the pairing page's own probe is refused every two seconds, by design", () => {
    for (const kind of ["checking", "none", "pending", "ended", "unreachable", "clock"]) {
      expect(shouldRereadOnRefusal(kind, false), kind).toBe(false);
    }
  });
});
