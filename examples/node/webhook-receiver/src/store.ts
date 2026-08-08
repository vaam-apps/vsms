/**
 * In-memory state for the demo. A real integration would back this with a
 * database — the point being demonstrated here is the *shape* of correct
 * handling, not a storage choice.
 */

export interface AggregateState {
  eventType: string;
  rank: number;
  occurredAt: string;
  updatedAt: string;
}

/**
 * Coarse precedence used only to decide whether a late-arriving event may
 * overwrite what this receiver currently reports as an aggregate's state.
 * This is NOT the real message state machine (`docs/architecture.md`'s
 * `message_state_transitions`) — it only needs to separate "still in
 * flight" from "settled," which is exactly what's needed to survive the
 * documented failure mode: `message.delivered` arriving before
 * `message.submitted` (§8.5: "Receivers must tolerate `message.delivered`
 * arriving before `message.submitted`").
 */
const EVENT_RANK: Readonly<Record<string, number>> = {
  "message.accepted": 0,
  "message.submitted": 1,
  "message.delivered": 2,
  "message.failed": 2,
  "message.expired": 2,
  "message.uncertain": 2,
  "message.cancelled": 2,
};

function rankOf(eventType: string): number {
  return EVENT_RANK[eventType] ?? 0;
}

export class WebhookStore {
  // Dedupe tuple, mirroring `webhook_attempts`' own unique index shape,
  // (endpoint_id, aggregate_id, event_type) — see §2.10 and §8.3. This
  // receiver serves exactly one endpoint, so endpoint_id is constant here
  // and dropped from the key rather than threaded through for no reason.
  private readonly seenTuples = new Set<string>();
  private readonly aggregates = new Map<string, AggregateState>();

  private tupleKey(aggregateId: string, eventType: string): string {
    return `${aggregateId}::${eventType}`;
  }

  /**
   * True the first time this (aggregateId, eventType) tuple is seen, false
   * on every repeat. At-least-once delivery (§8.2/§8.5) means the same
   * tuple WILL arrive again — a retried attempt, a replay from the admin
   * console (§8.5), or (before dedupe even reaches this receiver)
   * `Message.updated` firing on every touch server-side. Keying on
   * `sourceEventId` instead would treat each of those as new; the design
   * doc is explicit that aggregate + derived type is the correct tuple
   * (§2.10, §8.3), so this receiver uses the same one rather than
   * inventing its own.
   */
  recordIfNew(aggregateId: string, eventType: string): boolean {
    const key = this.tupleKey(aggregateId, eventType);
    if (this.seenTuples.has(key)) return false;
    this.seenTuples.add(key);
    return true;
  }

  /**
   * Applies an event to the tracked state for an aggregate, refusing to let
   * a lower-rank (more transient) event regress state that a higher-rank
   * one already reached. Returns `applied: false` when the event was
   * accepted (it's not an error — a late `submitted` after `delivered` is
   * a real, valid event) but didn't change what's displayed as current.
   */
  applyOrdered(
    aggregateId: string,
    eventType: string,
    occurredAt: string,
  ): { applied: boolean; current: AggregateState } {
    const incomingRank = rankOf(eventType);
    const existing = this.aggregates.get(aggregateId);

    if (!existing || incomingRank >= existing.rank) {
      const next: AggregateState = {
        eventType,
        rank: incomingRank,
        occurredAt,
        updatedAt: new Date().toISOString(),
      };
      this.aggregates.set(aggregateId, next);
      return { applied: true, current: next };
    }

    return { applied: false, current: existing };
  }

  get(aggregateId: string): AggregateState | undefined {
    return this.aggregates.get(aggregateId);
  }

  get aggregateCount(): number {
    return this.aggregates.size;
  }
}
