import "server-only";

// The T10 stream hub — **polling, permanently**, per DECISIONS §3 of the
// architecture plan (2026-08-08): no SSE, no `LISTEN`/`NOTIFY`, no Postgres
// trigger, no Rust changes. `MessageStreamHub` is a process-wide singleton:
// N callers share exactly ONE upstream poll loop against sms-gateway,
// regardless of how many times `subscribe()` is called.
//
// **Label this honestly everywhere it's described: this is polling with a
// streaming interface, not streaming.** Median latency ≈ `pollMs`.
// `packages/hooks/src/provider.tsx`'s own module doc already commits to
// this — the browser never gets a real subscription transport
// (`httpSubscriptionLink`/SSE), only `httpBatchStreamLink` — so
// `packages/api/src/routers/messages.ts`'s `onStateChange` wraps this
// hub's `subscribe()` in a short, bounded server-side wait (a plain HTTP
// long-poll) rather than holding a connection open to the browser. See
// that router's own module doc for exactly how, and why that's a
// deliberate, load-bearing consequence of this hub's design, not an
// afterthought.
//
// # The PII control (not an optimisation)
//
// The fixed browser contract emits exactly: `id, appId, state,
// stateReason, operator, segments, version, occurredAt,
// providerMessageRef`. **No `body`. No `msisdn`.** `@pii`/`@sensitive`
// redact audit snapshots only — sms-api's REST route still returns those
// fields to anyone who asks for them (verified live, `messages.ts`'s own
// module doc point 5). This module enforces the projection itself, via
// `fields=` on its own upstream request — the request never even carries
// those columns across the wire from sms-api, let alone to a browser tab.
// `message-stream.test.ts` asserts this mechanically: it renders every
// event this hub can emit and fails if any object literal contains a
// `body` or `msisdn` key.
//
// # Dedupe: `(id, version)`, not a timestamp
//
// `Message.version` is `@version` and increments on every update — an
// exact change key immune to clock skew and same-millisecond updates
// (AGENTS.md's own reasoning for why the claim loop and every other
// CAS-based mechanism in this codebase uses `@version` the same way). A
// bounded map (`SEEN_CAP` entries, oldest evicted first) remembers which
// `(id, version)` pairs have already been emitted.
//
// # Lifecycle and backoff
//
// One `setInterval` at `pollMs` (floor 500, `MESSAGE_STREAM_POLL_MS`,
// default 2000), started lazily when the subscriber count goes 0 → 1,
// stopped when it goes back to 0. A poll tick that arrives before the
// next allowed poll time (see `nextAllowedPollAt`) is a no-op rather than
// a fetch — this is what keeps "one poll per interval" true even under
// subscribe/unsubscribe churn (the bounded long-poll in
// `routers/messages.ts` subscribes and unsubscribes on every single
// browser request), not just under a single long-lived subscriber. On an
// upstream failure: exponential backoff 2s → 30s, a `degraded` frame to
// every subscriber (once, on the transition into the degraded state, not
// every failed tick), and the subscription stays open — never torn down
// on error. A `recovered` frame follows the first successful poll after a
// degraded spell, even if that poll had no new events to report, so a
// connection-loss UI bar (design doc §6.5 rule 8) has something to clear
// itself on beyond "an event happened to arrive."

import type { MessageState, OperatorCode } from "./client";
import { listMessagesForStream, type StreamCandidate } from "./messages";

export interface MessageStateEvent {
  type: "message";
  id: string;
  appId: string;
  state: MessageState;
  stateReason: string | null;
  operator: OperatorCode;
  segments: number;
  version: number;
  /** The design doc's fixed contract calls this `occurredAt`; `Message`
   * has no such column. Mapped from `updatedAt` — the field the state-
   * machine trigger actually bumps on every transition (see
   * `messages_guard_transition` in `0002_bootstrap/up.sql`), so it is the
   * real timestamp of "when this state took effect," just under sms-api's
   * own name for it. */
  occurredAt: string;
  providerMessageRef: string | null;
}

export interface MessageStreamDegradedEvent {
  type: "degraded";
  retryInMs: number;
}

export interface MessageStreamRecoveredEvent {
  type: "recovered";
}

export type MessageStreamFrame =
  | MessageStateEvent
  | MessageStreamDegradedEvent
  | MessageStreamRecoveredEvent;

export interface MessageStreamFilter {
  /** Only these states are delivered to this subscriber. Omit for all. */
  states?: MessageState[] | undefined;
}

const SEEN_CAP = 2000;
const BASE_BACKOFF_MS = 2000;
const MAX_BACKOFF_MS = 30_000;
const STREAM_WINDOW = 200;

interface Subscriber {
  filter: MessageStreamFilter;
  queue: MessageStreamFrame[];
  wake: (() => void) | null;
}

export interface MessageStreamHubOptions {
  pollMs: number;
  /** Injected so tests can control exactly what "upstream" returns and
   * count how many times it was actually called — see `start()`'s own
   * doc for why a poll tick and an upstream fetch are not the same thing. */
  fetchWindow: () => Promise<StreamCandidate[]>;
}

function toEvent(row: StreamCandidate): MessageStateEvent {
  return {
    type: "message",
    id: row.id,
    appId: row.appId,
    state: row.state,
    stateReason: row.stateReason ?? null,
    operator: row.operator,
    segments: row.segments,
    version: row.version,
    occurredAt: row.updatedAt,
    providerMessageRef: row.providerMessageRef ?? null,
  };
}

export class MessageStreamHub {
  private readonly pollMs: number;
  private readonly fetchWindow: () => Promise<StreamCandidate[]>;
  private readonly subscribers = new Set<Subscriber>();
  private readonly seen = new Map<string, true>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private nextAllowedPollAt = 0;
  private consecutiveFailures = 0;
  private degraded = false;

  constructor(options: MessageStreamHubOptions) {
    this.pollMs = options.pollMs;
    this.fetchWindow = options.fetchWindow;
  }

  /** True while the interval is running — i.e. at least one subscriber is
   * active. Exposed for tests ("last unsubscribe stops it"); nothing in
   * production reads this. */
  get isPolling(): boolean {
    return this.timer !== null;
  }

  get subscriberCount(): number {
    return this.subscribers.size;
  }

  /**
   * The hub's primary, literal API — an `AsyncIterable` per the
   * architecture plan's fixed contract. Registers a subscriber on first
   * iteration and unregisters it when the generator exits (`signal`
   * aborts, or the caller stops iterating and lets the generator be
   * garbage-collected/`return()`-ed by a `for await...of` `break`).
   */
  subscribe(filter: MessageStreamFilter, signal: AbortSignal): AsyncIterable<MessageStreamFrame> {
    const hub = this;
    async function* generator(): AsyncGenerator<MessageStreamFrame> {
      const subscriber: Subscriber = { filter, queue: [], wake: null };
      hub.addSubscriber(subscriber);
      try {
        while (!signal.aborted) {
          const next = subscriber.queue.shift();
          if (next !== undefined) {
            yield next;
            continue;
          }
          await new Promise<void>((resolve) => {
            subscriber.wake = resolve;
            if (signal.aborted) resolve();
            else signal.addEventListener("abort", () => resolve(), { once: true });
          });
        }
      } finally {
        hub.removeSubscriber(subscriber);
      }
    }
    return generator();
  }

  private addSubscriber(subscriber: Subscriber): void {
    this.subscribers.add(subscriber);
    if (this.subscribers.size === 1) this.start();
  }

  private removeSubscriber(subscriber: Subscriber): void {
    this.subscribers.delete(subscriber);
    if (this.subscribers.size === 0) this.stop();
  }

  private start(): void {
    // Allow an immediate poll for a fresh first subscriber rather than
    // waiting a full `pollMs` — `poll()`'s own `nextAllowedPollAt` gate
    // still protects against a burst of redundant fetches if `start()` is
    // re-entered quickly by subscribe/unsubscribe churn.
    this.nextAllowedPollAt = 0;
    void this.poll();
    this.timer = setInterval(() => void this.poll(), this.pollMs);
  }

  private stop(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
  }

  private async poll(): Promise<void> {
    if (Date.now() < this.nextAllowedPollAt) return;

    try {
      const rows = await this.fetchWindow();
      this.nextAllowedPollAt = Date.now() + this.pollMs;
      const wasDegraded = this.degraded;
      this.consecutiveFailures = 0;
      this.degraded = false;
      if (wasDegraded) this.broadcast({ type: "recovered" });

      for (const row of rows) {
        const key = `${row.id}:${row.version}`;
        if (this.seen.has(key)) continue;
        this.remember(key);
        this.broadcast(toEvent(row));
      }
    } catch {
      this.consecutiveFailures += 1;
      const backoff = Math.min(
        MAX_BACKOFF_MS,
        BASE_BACKOFF_MS * 2 ** (this.consecutiveFailures - 1),
      );
      this.nextAllowedPollAt = Date.now() + backoff;
      if (!this.degraded) {
        this.degraded = true;
        this.broadcast({ type: "degraded", retryInMs: backoff });
      }
    }
  }

  private remember(key: string): void {
    this.seen.set(key, true);
    if (this.seen.size > SEEN_CAP) {
      const oldest = this.seen.keys().next().value;
      if (oldest !== undefined) this.seen.delete(oldest);
    }
  }

  private broadcast(frame: MessageStreamFrame): void {
    for (const subscriber of this.subscribers) {
      if (frame.type === "message" && !matchesFilter(frame, subscriber.filter)) continue;
      subscriber.queue.push(frame);
      subscriber.wake?.();
      subscriber.wake = null;
    }
  }
}

function matchesFilter(event: MessageStateEvent, filter: MessageStreamFilter): boolean {
  if (filter.states === undefined) return true;
  return filter.states.includes(event.state);
}

declare global {
  // eslint-disable-next-line no-var
  var __vsmsMessageStreamHub: MessageStreamHub | undefined;
}

/**
 * The process-wide singleton. Cached on `globalThis` for the same reason
 * `dispatcher.ts` caches its `Agent` there — Next's dev-mode HMR
 * re-evaluates modules on every edit, and a module-level `const` would
 * silently spin up a second hub (a second `setInterval`, a second
 * upstream poller) per edit rather than reusing the one already running.
 */
export function getMessageStreamHub(pollMs: number): MessageStreamHub {
  globalThis.__vsmsMessageStreamHub ??= new MessageStreamHub({
    pollMs,
    fetchWindow: () => listMessagesForStream(STREAM_WINDOW),
  });
  return globalThis.__vsmsMessageStreamHub;
}
