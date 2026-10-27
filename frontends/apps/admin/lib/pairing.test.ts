// The pairing QR: the exact bytes the phone will relay to `registerConsoleKey`.
//
// The signature is checked the way the gateway checks it
// (`backends/crates/sms-api/src/console_keys.rs`): ECDSA P-256 over SHA-256 of
// `"vsms.console-pair.v1\n" + k + "\n" + t`, 64 raw bytes `r || s`, by an
// implementation that is not the WebCrypto code that made it (Node's `crypto`
// verifier, from the public point alone).

import { createPublicKey, verify } from "node:crypto";
import { describe, expect, it, vi } from "vitest";
import { loadOrCreateDeviceKey, memoryKeyStore } from "./device-key";
import {
  base64Url,
  browserLabel,
  buildPairingQr,
  cleanLabel,
  LABEL_MAX_CHARS,
  possessionMessage,
  QR_LIFETIME_SECONDS,
  QR_REFRESH_SECONDS,
  waitForApproval,
} from "./pairing";

/** What the gateway does with `publicKey`, `issuedAt` and `possessionProof`. */
function gatewayAccepts(
  fields: { k: string; t: number; s: string },
  now: number,
): "accepted" | "stale" | "bad key" | "bad signature" {
  const point = Buffer.from(fields.k, "base64url");
  if (point.length !== 65 || point[0] !== 0x04) return "bad key";
  if (Math.abs(now - fields.t) > QR_LIFETIME_SECONDS) return "stale";
  const key = createPublicKey({
    key: {
      kty: "EC",
      crv: "P-256",
      x: point.subarray(1, 33).toString("base64url"),
      y: point.subarray(33, 65).toString("base64url"),
    },
    format: "jwk",
  });
  const message = Buffer.from(`vsms.console-pair.v1\n${fields.k}\n${fields.t}`, "utf8");
  const signature = Buffer.from(fields.s, "base64url");
  return signature.length === 64 &&
    verify("sha256", message, { key, dsaEncoding: "ieee-p1363" }, signature)
    ? "accepted"
    : "bad signature";
}

async function qrFor(label: string, issuedAt: number) {
  const key = await loadOrCreateDeviceKey(memoryKeyStore());
  const qr = await buildPairingQr(key, label, issuedAt);
  return { key, qr, fields: JSON.parse(qr.text) as Record<string, unknown> };
}

describe("the pairing QR", () => {
  it("is vaam's admin-pairing format: v, k, t, l, s and nothing else", async () => {
    const { key, fields } = await qrFor("Chrome on macOS", 1_700_000_000);

    expect(Object.keys(fields)).toEqual(["v", "k", "t", "l", "s"]);
    expect(fields.v).toBe(1);
    expect(fields.k).toBe(base64Url(key.publicKey));
    expect(fields.t).toBe(1_700_000_000);
    expect(fields.l).toBe("Chrome on macOS");
    expect(typeof fields.s).toBe("string");
  });

  it("carries the 65-byte uncompressed point, unpadded base64url", async () => {
    const { fields } = await qrFor("Browser", 1_700_000_000);

    expect(fields.k).toMatch(/^[A-Za-z0-9_-]{87}$/);
    const point = Buffer.from(fields.k as string, "base64url");
    expect(point).toHaveLength(65);
    expect(point[0]).toBe(0x04);
  });

  it("is signed over the vsms.console-pair.v1 message, and verifies the way the gateway verifies it", async () => {
    const issuedAt = 1_700_000_000;
    const { fields } = await qrFor("Chrome on macOS", issuedAt);

    expect(gatewayAccepts(fields as { k: string; t: number; s: string }, issuedAt + 10)).toBe(
      "accepted",
    );
  });

  it("signs exactly the documented bytes", () => {
    expect(new TextDecoder().decode(possessionMessage("a2V5", 1_700_000_000))).toBe(
      "vsms.console-pair.v1\na2V5\n1700000000",
    );
  });

  it("does not stand for vaam's own admin-pairing signature, or for any other tag", async () => {
    const { key, fields } = await qrFor("Browser", 1_700_000_000);
    const k = fields.k as string;
    // The same key signing vaam's message (which has the label in it) is not a
    // possession proof for vsms.
    const vaamMessage = new TextEncoder().encode(`vaam.admin-pair.v1\n${k}\n1700000000\nBrowser`);
    const s = base64Url(await key.sign(vaamMessage));

    expect(gatewayAccepts({ k, t: 1_700_000_000, s }, 1_700_000_000)).toBe("bad signature");
  });

  it("is refused when its time or its key was changed after signing", async () => {
    const { fields } = await qrFor("Browser", 1_700_000_000);
    const original = fields as { k: string; t: number; s: string };

    expect(gatewayAccepts({ ...original, t: original.t + 1 }, original.t)).toBe("bad signature");
    const other = await qrFor("Browser", 1_700_000_000);
    expect(gatewayAccepts({ ...original, k: (other.fields as { k: string }).k }, original.t)).toBe(
      "bad signature",
    );
  });

  it("is stale after five minutes either side, as the gateway counts it", async () => {
    const { fields } = await qrFor("Browser", 1_700_000_000);
    const qr = fields as { k: string; t: number; s: string };

    expect(gatewayAccepts(qr, qr.t + QR_LIFETIME_SECONDS)).toBe("accepted");
    expect(gatewayAccepts(qr, qr.t + QR_LIFETIME_SECONDS + 1)).toBe("stale");
    expect(gatewayAccepts(qr, qr.t - QR_LIFETIME_SECONDS - 1)).toBe("stale");
    // And it is redrawn well before that.
    expect(QR_REFRESH_SECONDS).toBeLessThan(QR_LIFETIME_SECONDS);
  });

  it("does not sign the label: the gateway's message has no slot for it", async () => {
    // The label is the browser's own unauthenticated claim, shown by the phone
    // as such. Changing it does not break the proof, which is why nothing may
    // treat it as verified.
    const a = await qrFor("Chrome on macOS", 1_700_000_000);
    const parsed = a.fields as { k: string; t: number; s: string };

    expect(gatewayAccepts({ ...parsed }, 1_700_000_000)).toBe("accepted");
  });
});

describe("the label", () => {
  it("is printable ASCII, single-spaced and at most 40 characters", () => {
    expect(cleanLabel("  Chromeé   on\tmacOS ")).toBe("Chrome on macOS");
    expect(cleanLabel("x".repeat(100))).toHaveLength(LABEL_MAX_CHARS);
    expect(cleanLabel("   ")).toBe("Browser");
  });

  it("names the browser and the platform it reports", () => {
    expect(
      browserLabel({
        userAgentData: {
          brands: [{ brand: "Not A;Brand" }, { brand: "Chromium" }, { brand: "Google Chrome" }],
          platform: "macOS",
        },
      }),
    ).toBe("Google Chrome on macOS");
    expect(browserLabel({ userAgent: "Mozilla/5.0 Firefox/130.0", platform: "Linux x86_64" })).toBe(
      "Firefox on Linux x86_64",
    );
  });
});

describe("waiting for the gateway to accept the key", () => {
  const instantly = () => Promise.resolve();

  it("treats every failure as 'not yet' and ends only on a success", async () => {
    const check = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce({ httpStatus: 401 })
      .mockRejectedValueOnce(new Error("network"))
      .mockResolvedValueOnce(undefined);

    const accepted = await waitForApproval({ check, sleep: instantly });

    expect(accepted).toBe(true);
    expect(check).toHaveBeenCalledTimes(3);
  });

  it("stops when the QR on screen has expired, so a new one can be drawn", async () => {
    let expired = false;
    const check = vi.fn(async () => {
      expired = true;
      throw { httpStatus: 401 };
    });

    const accepted = await waitForApproval({ check, sleep: instantly, expired: () => expired });

    expect(accepted).toBe(false);
    expect(check).toHaveBeenCalledTimes(1);
  });

  it("stops when aborted, and rethrows what is fatal", async () => {
    const controller = new AbortController();
    controller.abort();
    expect(await waitForApproval({ check: vi.fn(), signal: controller.signal })).toBe(false);

    await expect(
      waitForApproval({
        check: async () => {
          throw new Error("spent");
        },
        fatal: () => true,
        sleep: instantly,
      }),
    ).rejects.toThrow("spent");
  });
});
