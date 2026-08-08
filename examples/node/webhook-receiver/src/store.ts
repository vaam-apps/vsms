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

export type DuplicateReason = "event-id" | "aggregate-tuple" | null;

export class WebhookStore {
  // PRIMARY dedupe key, and the documented receiver contract: §4.4 says,
  // in so many words, "Send X-Sms-Event-Id and mean it — delivery is
  // at-least-once and receivers need a dedupe key." This is NOT the same
  // thing as `webhook_attempts`' own unique index — that index,
  // (endpoint_id, aggregate_id, event_type) (§2.10), is vsms's *sender-side*
  // guard against creating duplicate WebhookAttempt rows in the first
  // place. It says nothing about what a *receiver* should key on once
  // at-least-once HTTP delivery (retries, replays — §8.5) reaches it. §4.4
  // does, and it names `X-Sms-Event-Id`.
  private readonly seenEventIds = new Set<string>();

  // SECONDARY, defensive, and explicitly NOT part of §4.4's contract: also
  // recognise a duplicate by (aggregateId, eventType) even when the event
  // id differs. Nothing in §4.4 says a retry reuses the same event id —
  // if a real sender ever regenerated one per retry, keying only on
  // `X-Sms-Event-Id` would silently reprocess it. This mirrors
  // `webhook_attempts`' own dedupe shape (§2.10, §8.3) and is strictly
  // stronger than the documented contract, kept as belt-and-braces on top
  // of it rather than instead of it.
  private readonly seenTuples = new Set<string>();
  private readonly aggregates = new Map<string, AggregateState>();

  private tupleKey(aggregateId: string, eventType: string): string {
    return `${aggregateId}::${eventType}`;
  }

  /**
   * Returns which check caught a duplicate, or `null` if this is genuinely
   * new. Checked, not recorded — see `recordSeen`, called separately so
   * the caller can decide whether to actually do the "processing" a
   * duplicate should skip.
   */
  checkDuplicate(eventId: string, aggregateId: string, eventType: string): DuplicateReason {
    if (this.seenEventIds.has(eventId)) return "event-id";
    if (this.seenTuples.has(this.tupleKey(aggregateId, eventType))) return "aggregate-tuple";
    return null;
  }

  /** Records both keys for a newly-processed event. Call only after
   *  `checkDuplicate` returned `null` for the same arguments. */
  recordSeen(eventId: string, aggregateId: string, eventType: string): void {
    this.seenEventIds.add(eventId);
    this.seenTuples.add(this.tupleKey(aggregateId, eventType));
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
