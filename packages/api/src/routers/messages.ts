import "server-only";

// `messages.list` / `messages.byId` / `messages.onStateChange` — T10/T12.
//
// `list`/`byId` are thin wraps over `@vsms/gateway`'s `listMessages`/
// `getMessageById` (see that module's own doc for the live query-grammar
// probe behind their shape — `GET /messages` / `GET /messages/{id}`, not
// the unimplemented `listMessagesPage` procedure).
//
// `onStateChange` is the one genuinely non-obvious piece. The fixed
// browser contract in the architecture plan describes it as returning an
// `AsyncIterable<MessageStateEvent>`, which is exactly what
// `MessageStreamHub.subscribe()` (`@vsms/gateway/message-stream.ts`)
// really is — but `packages/hooks/src/provider.tsx`'s own module doc
// already commits this codebase to `httpBatchStreamLink` only, no
// `httpSubscriptionLink`/SSE, "even once T10 lands." A tRPC
// `.subscription()` procedure requires a subscription-capable link on the
// client, which this app deliberately doesn't have. So this procedure is
// a plain `.query()` that performs a **bounded server-side long-poll**:
// it opens a real hub subscription, waits up to
// `MESSAGE_STREAM_POLL_MS` for at least one frame (draining whatever
// else is immediately available too), then closes the subscription and
// returns what it collected — possibly nothing, if nothing changed in
// that window. The browser drives this with `useQuery`'s own
// `refetchInterval`, not a live connection.
//
// This means the hub's `subscribe()`/`unsubscribe()` lifecycle churns
// once per browser poll rather than staying open for a tab's whole
// lifetime — an honest tradeoff of the "no SSE" decision, not a bug: the
// hub's own `nextAllowedPollAt` throttle (see its module doc) is what
// keeps the upstream fetch itself to at most one per `MESSAGE_STREAM_POLL_MS`
// regardless of how often subscriptions open and close, which is the
// property that actually matters for sms-api's load, not whether the
// Node-level subscriber handle happens to be long- or short-lived.

import { TRPCError } from "@trpc/server";
import { env } from "@vsms/env";
import type { MessageStreamFrame } from "@vsms/gateway";
import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const messageState = z.enum([
  "accepted",
  "queued",
  "routed",
  "submitted",
  "delivered",
  "uncertain",
  "undelivered",
  "failed",
  "expired",
  "rejected",
  "cancelled",
]);

const listInput = z.object({
  state: messageState.optional(),
  clientRef: z.string().min(1).optional(),
  from: z.string().datetime().optional(),
  to: z.string().datetime().optional(),
  limit: z.number().int().positive().max(200).optional(),
  offset: z.number().int().nonnegative().optional(),
  sort: z.enum(["createdAt", "-createdAt"]).optional(),
});

const byIdInput = z.object({ id: z.string().min(1) });

const onStateChangeInput = z.object({
  states: z.array(messageState).optional(),
});

/** Safety cap, not a realistic ceiling — a single `MESSAGE_STREAM_POLL_MS`
 * window should never accumulate more than a handful of real state
 * changes; this only bounds the pathological case (e.g. a very stale
 * client resubscribing after a long gap). */
const MAX_FRAMES_PER_LONG_POLL = 200;

export const messagesRouter = router({
  list: publicProcedure.input(listInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.listMessages(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  byId: publicProcedure.input(byIdInput).query(async ({ ctx, input }) => {
    let record: Awaited<ReturnType<typeof ctx.gateway.getMessageById>>;
    try {
      record = await ctx.gateway.getMessageById(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
    if (record === null) {
      // sms-api itself can't distinguish "never existed" from "exists but
      // belongs to another app" (@vsms/gateway/messages.ts's own doc,
      // point 9) — this procedure doesn't pretend to either.
      throw new TRPCError({ code: "NOT_FOUND", message: "Message not found" });
    }
    return record;
  }),

  // #50: the detail screen's timeline evidence — `DeliveryReceipt` rows
  // for one message, via `POST /$procs/listMessageReceipts`
  // (`@vsms/gateway`'s own module doc explains why that procedure exists
  // rather than a REST route). An empty `receipts` array is a normal,
  // expected outcome, not an error — the detail screen's own
  // `buildTimeline` (`admin/app/messages/[id]/timeline.ts`) is what turns
  // "zero receipts" into an honest "the outcome was never learned"
  // annotation for a message sitting in `uncertain`, rather than this
  // procedure inventing one.
  receipts: publicProcedure.input(byIdInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.listMessageReceipts(input.id);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  onStateChange: publicProcedure.input(onStateChangeInput).query(async ({ ctx, input }) => {
    const hub = ctx.gateway.getMessageStreamHub(env.MESSAGE_STREAM_POLL_MS);
    const controller = new AbortController();
    const filter = input.states !== undefined ? { states: input.states } : {};
    const iterator = hub.subscribe(filter, controller.signal)[Symbol.asyncIterator]();

    const frames: MessageStreamFrame[] = [];
    try {
      const deadline = Date.now() + env.MESSAGE_STREAM_POLL_MS;
      while (frames.length < MAX_FRAMES_PER_LONG_POLL) {
        const remainingMs = deadline - Date.now();
        if (remainingMs <= 0) break;

        const timedOut = Symbol("timed-out");
        const outcome = await Promise.race([
          iterator.next(),
          new Promise<typeof timedOut>((resolve) => {
            setTimeout(() => resolve(timedOut), remainingMs);
          }),
        ]);

        if (outcome === timedOut) break;
        if (outcome.done) break;
        frames.push(outcome.value);
      }
    } finally {
      // Unblocks the generator's pending wait (if any) and drives its own
      // `finally` — unregistering this subscriber from the hub. See this
      // file's own module doc for why a subscription's lifetime is one
      // long-poll, not one browser tab.
      controller.abort();
    }

    return { frames, degraded: frames.some((frame) => frame.type === "degraded") };
  }),
});
