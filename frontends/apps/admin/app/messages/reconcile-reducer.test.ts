import { describe, expect, it } from "vitest";
import type { MessageListItem, MessageStreamEvent } from "./apply-event";
import { INITIAL_RECONCILE_STATE, reconcileReducer } from "./reconcile-reducer";

function row(overrides: Partial<MessageListItem> = {}): MessageListItem {
  return {
    id: "row-a",
    appId: "app-1",
    msisdn: "+237677123456",
    operator: "mtn",
    senderIdValue: "ACME",
    class: "transactional",
    state: "accepted",
    encoding: "gsm7",
    segments: 1,
    version: 1,
    createdAt: "2026-08-08T14:00:00.000Z",
    updatedAt: "2026-08-08T14:00:00.000Z",
    ...overrides,
  };
}

function event(overrides: Partial<MessageStreamEvent> = {}): MessageStreamEvent {
  return {
    type: "message",
    id: "row-a",
    appId: "app-1",
    state: "queued",
    stateReason: null,
    operator: "mtn",
    segments: 1,
    version: 2,
    occurredAt: "2026-08-08T14:00:05.000Z",
    providerMessageRef: null,
    ...overrides,
  };
}

describe("reconcileReducer", () => {
  it("starts empty", () => {
    expect(INITIAL_RECONCILE_STATE).toEqual({ rows: [], pending: [] });
  });

  it("'reset' reseeds rows and clears pending", () => {
    const withPending = reconcileReducer(INITIAL_RECONCILE_STATE, {
      type: "event",
      event: event({ id: "buffered" }),
      stateFilter: null,
      scrolledAway: true,
    });
    expect(withPending.pending).toHaveLength(1);

    const items = [row({ id: "fresh" })];
    const result = reconcileReducer(withPending, { type: "reset", items });
    expect(result).toEqual({ rows: items, pending: [] });
  });

  it("'event' delegates to applyEvent's own merge rules", () => {
    const result = reconcileReducer(INITIAL_RECONCILE_STATE, {
      type: "event",
      event: event({ id: "new-row" }),
      stateFilter: null,
      scrolledAway: false,
    });
    expect(result.rows.map((r) => r.id)).toEqual(["new-row"]);
  });

  it("'insertPending' flushes the buffer into rows", () => {
    const buffered = reconcileReducer(INITIAL_RECONCILE_STATE, {
      type: "event",
      event: event({ id: "buffered" }),
      stateFilter: null,
      scrolledAway: true,
    });
    expect(buffered.rows).toEqual([]);
    expect(buffered.pending).toHaveLength(1);

    const flushed = reconcileReducer(buffered, { type: "insertPending" });
    expect(flushed.pending).toEqual([]);
    expect(flushed.rows.map((r) => r.id)).toEqual(["buffered"]);
  });
});
