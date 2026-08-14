// Unit tests for `MessageStreamHub` — run directly against the hub, not
// through HTTP/tRPC (see `message-stream.ts`'s own module doc and this
// task's brief: "assert mechanically: two subscribers ⇒ one poll per
// interval; last unsubscribe stops it"). `fetchWindow` is injected so
// these tests control exactly what "upstream" returns and can count real
// calls, without a live gateway.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type MessageStreamFrame, MessageStreamHub } from "./message-stream";
import type { StreamCandidate } from "./messages";

const POLL_MS = 2000;

function row(overrides: Partial<StreamCandidate> = {}): StreamCandidate {
  return {
    id: "msg1",
    appId: "app1",
    state: "queued",
    operator: "mtn",
    segments: 1,
    version: 1,
    updatedAt: "2026-08-08T00:00:00.000Z",
    ...overrides,
  };
}

/** Drains everything currently queued for a fresh subscription without
 * blocking on the next real event — used to assert "no new frame arrived"
 * without hanging the test. */
async function collectAvailable(
  hub: MessageStreamHub,
  signal: AbortSignal,
  count: number,
): Promise<MessageStreamFrame[]> {
  const out: MessageStreamFrame[] = [];
  for await (const frame of hub.subscribe({}, signal)) {
    out.push(frame);
    if (out.length >= count) break;
  }
  return out;
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("MessageStreamHub lifecycle", () => {
  it("starts polling lazily on the first subscriber and stops on the last unsubscribe", async () => {
    const fetchWindow = vi.fn().mockResolvedValue([]);
    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });

    expect(hub.isPolling).toBe(false);

    const controllerA = new AbortController();
    const iteratorA = hub.subscribe({}, controllerA.signal)[Symbol.asyncIterator]();
    const pendingA = iteratorA.next();
    await vi.advanceTimersByTimeAsync(0); // flush the eager first poll's microtasks
    expect(hub.subscriberCount).toBe(1);
    expect(hub.isPolling).toBe(true);

    const controllerB = new AbortController();
    const iteratorB = hub.subscribe({}, controllerB.signal)[Symbol.asyncIterator]();
    const pendingB = iteratorB.next();
    await vi.advanceTimersByTimeAsync(0);
    expect(hub.subscriberCount).toBe(2);

    controllerA.abort();
    await pendingA;
    expect(hub.isPolling).toBe(true); // one subscriber (B) is still active

    controllerB.abort();
    await pendingB;
    expect(hub.isPolling).toBe(false);
  });

  it("two concurrent subscribers still produce exactly one upstream fetch per interval", async () => {
    const fetchWindow = vi.fn().mockResolvedValue([]);
    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });

    const controllerA = new AbortController();
    const controllerB = new AbortController();
    void collectAvailable(hub, controllerA.signal, Number.POSITIVE_INFINITY).catch(() => {});
    void collectAvailable(hub, controllerB.signal, Number.POSITIVE_INFINITY).catch(() => {});

    // Flush the eager immediate poll triggered by the first subscriber —
    // both registrations happen synchronously above, before either's
    // `fetchWindow()` promise has a chance to resolve, so this is really
    // "two subscribers were both live before the first fetch completed."
    await vi.advanceTimersByTimeAsync(0);
    expect(hub.subscriberCount).toBe(2);
    expect(fetchWindow).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(fetchWindow).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(POLL_MS);
    expect(fetchWindow).toHaveBeenCalledTimes(3);

    controllerA.abort();
    controllerB.abort();
    await vi.advanceTimersByTimeAsync(0);
    expect(hub.isPolling).toBe(false);
  });

  it("dedupes on (id, version): the same pair is only ever emitted once", async () => {
    const fetchWindow = vi
      .fn()
      .mockResolvedValueOnce([row({ id: "m1", version: 1 })])
      .mockResolvedValueOnce([row({ id: "m1", version: 1 })]) // unchanged — must not re-emit
      .mockResolvedValueOnce([row({ id: "m1", version: 2 })]); // a real change — must emit

    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });
    const controller = new AbortController();
    const events = collectAvailable(hub, controller.signal, 2);

    await vi.advanceTimersByTimeAsync(POLL_MS); // tick 2 (unchanged, no new event)
    await vi.advanceTimersByTimeAsync(POLL_MS); // tick 3 (version bumped)

    const frames = await events;
    expect(frames).toHaveLength(2);
    expect(frames[0]).toMatchObject({ type: "message", id: "m1", version: 1 });
    expect(frames[1]).toMatchObject({ type: "message", id: "m1", version: 2 });

    controller.abort();
  });

  it("filters by state when a subscriber asks for a subset", async () => {
    const fetchWindow = vi
      .fn()
      .mockResolvedValueOnce([row({ id: "a", state: "queued", version: 1 })])
      .mockResolvedValueOnce([row({ id: "b", state: "delivered", version: 1 })]);

    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });
    const controller = new AbortController();
    const iterator = hub
      .subscribe({ states: ["delivered"] }, controller.signal)
      [Symbol.asyncIterator]();
    const pending = iterator.next();

    await vi.advanceTimersByTimeAsync(POLL_MS); // "a" (queued) — filtered out
    const result = await pending; // resolves once "b" (delivered) arrives
    expect(result.done).toBe(false);
    if (!result.done) {
      expect(result.value).toMatchObject({ id: "b", state: "delivered" });
    }

    controller.abort();
  });

  it("emits a single `degraded` frame on failure and `recovered` on the next success", async () => {
    const fetchWindow = vi
      .fn()
      .mockRejectedValueOnce(new Error("upstream down"))
      .mockRejectedValueOnce(new Error("still down"))
      .mockResolvedValueOnce([]);

    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });
    const controller = new AbortController();
    const events = collectAvailable(hub, controller.signal, 2);

    await vi.advanceTimersByTimeAsync(1); // let the eager first poll fail
    // Backoff after failure #1 is 2000ms (BASE_BACKOFF_MS); advance past it.
    await vi.advanceTimersByTimeAsync(2000);
    // Backoff after failure #2 is 4000ms; advance past it to the recovering poll.
    await vi.advanceTimersByTimeAsync(4000);

    const frames = await events;
    expect(frames[0]).toEqual({ type: "degraded", retryInMs: 2000 });
    expect(frames[1]).toEqual({ type: "recovered" });

    controller.abort();
  });

  it("never emits an event object containing a body or msisdn key", async () => {
    const fetchWindow = vi
      .fn()
      .mockResolvedValue([
        row({ id: "m1", version: 1 }),
        row({ id: "m2", version: 1, stateReason: "no active provider" }),
        row({ id: "m3", version: 1, providerMessageRef: "orange-ref-123" }),
      ]);

    const hub = new MessageStreamHub({ pollMs: POLL_MS, fetchWindow });
    const controller = new AbortController();
    const frames = await collectAvailable(hub, controller.signal, 3);
    controller.abort();

    expect(frames).toHaveLength(3);
    for (const frame of frames) {
      expect(Object.keys(frame)).not.toContain("body");
      expect(Object.keys(frame)).not.toContain("msisdn");
      // Also assert the full allow-list — a stray extra key would be just
      // as much a PII leak as the two named ones if it were ever added by
      // accident to `StreamCandidate`/`toEvent`.
      expect(Object.keys(frame).sort()).toEqual(
        [
          "type",
          "id",
          "appId",
          "state",
          "stateReason",
          "operator",
          "segments",
          "version",
          "occurredAt",
          "providerMessageRef",
        ].sort(),
      );
    }
  });
});
