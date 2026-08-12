import { describe, expect, it } from "vitest";
import { buildTimeline, type TimelineMessageInput } from "./timeline";

function baseMessage(overrides: Partial<TimelineMessageInput> = {}): TimelineMessageInput {
  return {
    state: "accepted",
    createdAt: "2026-08-08T14:03:07.412Z",
    updatedAt: "2026-08-08T14:03:07.412Z",
    attempts: 0,
    maxAttempts: 3,
    ...overrides,
  };
}

describe("buildTimeline", () => {
  it("shows only 'accepted' for a freshly accepted message", () => {
    const result = buildTimeline(baseMessage());
    expect(result).toEqual([{ toState: "accepted", at: "2026-08-08T14:03:07.412Z" }]);
  });

  it("adds a submitted entry once submittedAt is known", () => {
    const result = buildTimeline(
      baseMessage({
        state: "submitted",
        submittedAt: "2026-08-08T14:03:08.312Z",
        updatedAt: "2026-08-08T14:03:08.312Z",
        attempts: 1,
      }),
    );
    expect(result).toEqual([
      { toState: "accepted", at: "2026-08-08T14:03:07.412Z" },
      { toState: "submitted", at: "2026-08-08T14:03:08.312Z", attempt: 1, maxAttempts: 3 },
    ]);
  });

  it("never fabricates queued/routed entries — no timestamp evidence exists for them", () => {
    // A message currently `routed` has neither a `queuedAt` nor a
    // `routedAt` column anywhere in this schema — `buildTimeline` must
    // not invent either hop, even though we know the message passed
    // through them logically.
    const result = buildTimeline(
      baseMessage({ state: "routed", updatedAt: "2026-08-08T14:03:08.010Z" }),
    );
    expect(result.map((t) => t.toState)).toEqual(["accepted", "routed"]);
  });

  it("reaches delivered cleanly with finalizedAt as the terminal timestamp", () => {
    const result = buildTimeline(
      baseMessage({
        state: "delivered",
        submittedAt: "2026-08-08T14:03:08.312Z",
        finalizedAt: "2026-08-08T14:03:38.500Z",
        updatedAt: "2026-08-08T14:03:38.500Z",
        attempts: 1,
      }),
    );
    expect(result).toEqual([
      { toState: "accepted", at: "2026-08-08T14:03:07.412Z" },
      { toState: "submitted", at: "2026-08-08T14:03:08.312Z", attempt: 1, maxAttempts: 3 },
      { toState: "delivered", at: "2026-08-08T14:03:38.500Z", attempt: 1, maxAttempts: 3 },
    ]);
  });

  // --- The guard #50 actually cares about. --------------------------
  //
  // A message that went `submitted -> uncertain` via
  // `ProviderError::Indeterminate` (crates/sms-provider's own taxonomy,
  // #119) has ZERO `DeliveryReceipt` rows — the outcome was never
  // learned, by construction (see `crates/sms-worker/tests/
  // chaos_live_postgres.rs`'s scripted scenarios). `buildTimeline` must
  // still surface `uncertain` as the current state — with no receipt to
  // point at — so `StateTimeline`'s own built-in annotation for
  // `uncertain` fires, rather than the timeline silently stopping at
  // `submitted` and looking like a stalled-but-otherwise-clean send.
  it("surfaces an Indeterminate-submit gap: uncertain with zero receipts still appears as the current state", () => {
    const result = buildTimeline(
      baseMessage({
        state: "uncertain",
        submittedAt: "2026-08-08T14:03:08.312Z",
        updatedAt: "2026-08-08T14:03:38.312Z",
        attempts: 1,
      }),
    );

    const finalEntry = result[result.length - 1];
    expect(finalEntry?.toState).toBe("uncertain");
    expect(finalEntry?.at).toBe("2026-08-08T14:03:38.312Z");
    // The whole point: this entry exists with NO receipt behind it. This
    // module never receives receipts as an input in the first place —
    // asserting that is the point of this test's existence, not an
    // afterthought: a future change that made `buildTimeline` require a
    // non-empty receipts array to show `uncertain` would silently regress
    // exactly the case #50 was written to fix.
    expect(result.map((t) => t.toState)).toEqual(["accepted", "submitted", "uncertain"]);
  });

  it("surfaces an undelivered gap (a retryable-failure DLR) as the current state", () => {
    const result = buildTimeline(
      baseMessage({
        state: "undelivered",
        submittedAt: "2026-08-08T14:03:08.312Z",
        updatedAt: "2026-08-08T14:03:12.900Z",
        attempts: 1,
      }),
    );
    const finalEntry = result[result.length - 1];
    expect(finalEntry?.toState).toBe("undelivered");
    expect(finalEntry?.at).toBe("2026-08-08T14:03:12.900Z");
  });

  // --- A bug this exact test caught live, driving a real message through
  // `just demo`, before this test existed. ---------------------------
  //
  // `sms-api`'s REST `GET /messages/{id}` sends `"submittedAt": null` — a
  // real JSON `null`, not an omitted key — for a message that went
  // `routed -> uncertain` directly via an `Indeterminate` submit (see
  // `timeline.ts`'s own module doc for the exact live response body). The
  // first version of `buildTimeline` checked `!== undefined`, which is
  // `true` for `null`, and rendered a bogus "Submitted" entry dated the
  // Unix epoch. This is the regression test for that fix, using the exact
  // shape (`null`, not `undefined`) the real response had.
  it("treats a JSON-null submittedAt exactly like an absent one — no epoch-dated 'submitted' entry", () => {
    const result = buildTimeline({
      state: "uncertain",
      createdAt: "2026-08-12T08:31:11.428781Z",
      submittedAt: null,
      finalizedAt: null,
      updatedAt: "2026-08-12T08:31:14.528534Z",
      attempts: 1,
      maxAttempts: 3,
    });
    expect(result.map((t) => t.toState)).toEqual(["accepted", "uncertain"]);
    expect(result.some((t) => t.at.startsWith("1970"))).toBe(false);
  });

  it("does not duplicate the current-state entry when the message hasn't moved past submitted", () => {
    const result = buildTimeline(
      baseMessage({
        state: "submitted",
        submittedAt: "2026-08-08T14:03:08.312Z",
        updatedAt: "2026-08-08T14:03:08.312Z",
        attempts: 1,
      }),
    );
    expect(result).toHaveLength(2);
    expect(result[1]?.toState).toBe("submitted");
  });
});
