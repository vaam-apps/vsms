import { describe, expect, it } from "vitest";
import {
  applyEvent,
  insertPendingIntoRows,
  type MessageListItem,
  type MessageStreamEvent,
  type ReconcileState,
  resetReconcileState,
} from "./apply-event";

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

function emptyState(overrides: Partial<ReconcileState> = {}): ReconcileState {
  return { rows: [], pending: [], ...overrides };
}

describe("applyEvent", () => {
  // Rule 3 (messages-screen.tsx's own module doc): in-place status change
  // never moves a row — an update always `.map()`s the existing array,
  // never removes/reinserts.
  it("merges an update to an existing row in place, without changing its position", () => {
    const first = row({ id: "row-a" });
    const second = row({ id: "row-b" });
    const state = emptyState({ rows: [first, second] });

    const result = applyEvent(
      state,
      event({ id: "row-b", state: "delivered", version: 3 }),
      null,
      false,
    );

    expect(result.rows.map((r) => r.id)).toEqual(["row-a", "row-b"]);
    expect(result.rows[1]?.state).toBe("delivered");
    expect(result.rows[1]?.version).toBe(3);
    expect(result.rows[0]).toBe(first); // untouched row is the exact same reference
  });

  it("merges an update to a row sitting in the pending buffer without moving it out of pending", () => {
    const pendingRow = row({ id: "row-p", state: "queued", version: 1 });
    const state = emptyState({ pending: [pendingRow] });

    const result = applyEvent(
      state,
      event({ id: "row-p", state: "routed", version: 2 }),
      null,
      true,
    );

    expect(result.rows).toEqual([]);
    expect(result.pending).toHaveLength(1);
    expect(result.pending[0]?.state).toBe("routed");
    expect(result.pending[0]?.version).toBe(2);
  });

  // Buffering is scroll-gated: at scroll-top a genuinely new row inserts
  // directly into `rows`; scrolled away, it buffers into `pending`.
  it("inserts a genuinely new row directly into rows when not scrolled away", () => {
    const result = applyEvent(emptyState(), event({ id: "row-new" }), null, false);
    expect(result.rows.map((r) => r.id)).toEqual(["row-new"]);
    expect(result.pending).toEqual([]);
  });

  it("buffers a genuinely new row into pending when scrolled away", () => {
    const result = applyEvent(emptyState(), event({ id: "row-new" }), null, true);
    expect(result.pending.map((r) => r.id)).toEqual(["row-new"]);
    expect(result.rows).toEqual([]);
  });

  it("prepends new rows/pending entries so the newest sorts first", () => {
    const existing = row({ id: "row-old" });
    const state = emptyState({ rows: [existing] });
    const result = applyEvent(state, event({ id: "row-new" }), null, false);
    expect(result.rows.map((r) => r.id)).toEqual(["row-new", "row-old"]);
  });

  it("matches every event when stateFilter is null, regardless of state", () => {
    const result = applyEvent(emptyState(), event({ state: "failed" }), null, false);
    expect(result.rows).toHaveLength(1);
  });

  it("drops a tracked row from rows once it transitions out of the active filter", () => {
    const tracked = row({ id: "row-a", state: "queued" });
    const state = emptyState({ rows: [tracked] });
    const result = applyEvent(state, event({ id: "row-a", state: "routed" }), "queued", false);
    expect(result.rows).toEqual([]);
    expect(result.pending).toEqual([]);
  });

  it("drops a tracked row from pending once it transitions out of the active filter", () => {
    const tracked = row({ id: "row-a", state: "queued" });
    const state = emptyState({ pending: [tracked] });
    const result = applyEvent(state, event({ id: "row-a", state: "routed" }), "queued", true);
    expect(result.rows).toEqual([]);
    expect(result.pending).toEqual([]);
  });

  it("is a no-op for an event that doesn't match the filter and isn't tracked in either list", () => {
    const state = emptyState();
    const result = applyEvent(state, event({ id: "row-unseen", state: "routed" }), "queued", false);
    expect(result).toBe(state); // same reference — a genuine no-op, not a same-shape copy
  });

  it("never fabricates a full row for a new event — unknown fields fall back to placeholders", () => {
    const result = applyEvent(emptyState(), event({ id: "row-new" }), null, false);
    expect(result.rows[0]).toMatchObject({
      msisdn: "",
      senderIdValue: "",
      class: "transactional",
      encoding: "gsm7",
    });
  });
});

describe("resetReconcileState", () => {
  it("seeds rows from the given items and clears any buffered pending rows", () => {
    const items = [row({ id: "a" }), row({ id: "b" })];
    const result = resetReconcileState(items);
    expect(result).toEqual({ rows: items, pending: [] });
  });
});

describe("insertPendingIntoRows", () => {
  it("moves every buffered row to the top of rows, newest first, and clears pending", () => {
    const existing = row({ id: "old" });
    const buffered = [row({ id: "new-2" }), row({ id: "new-1" })];
    const state: ReconcileState = { rows: [existing], pending: buffered };

    const result = insertPendingIntoRows(state);

    expect(result.rows.map((r) => r.id)).toEqual(["new-2", "new-1", "old"]);
    expect(result.pending).toEqual([]);
  });

  it("is a genuine no-op (same reference) when nothing is buffered", () => {
    const state = emptyState({ rows: [row()] });
    expect(insertPendingIntoRows(state)).toBe(state);
  });
});
