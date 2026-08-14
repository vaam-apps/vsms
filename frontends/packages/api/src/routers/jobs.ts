import "server-only";

// `jobs.list` / `jobs.requeue` — #56's Jobs screen. Thin wraps over
// `@vsms/gateway`'s `listJobs`/`requeueJob` (see that module's own doc for
// the live-grammar reasoning `listJobs` inherits from `messages.ts` rather
// than re-probing).
//
// No `onStateChange`-style live poll here, unlike `messages.ts` — there is
// no server-side hub for jobs, and this screen doesn't need one: a job
// backlog changes at the pace of `jobs::POLL_INTERVAL` (1s) at the
// fastest, and an operator diagnosing a stuck job is looking at a
// snapshot, not watching a live feed the way the messages list's own
// design doc (§6.5) asks for. `jobs-screen.tsx` uses a plain
// `refetchInterval` instead.

import { z } from "zod";
import { rethrowGatewayError } from "../gateway-error";
import { publicProcedure, router } from "../trpc";

const jobState = z.enum(["pending", "running", "succeeded", "failed", "dead", "cancelled"]);

const listInput = z.object({
  state: jobState.optional(),
  kind: z.string().min(1).optional(),
  limit: z.number().int().positive().max(500).optional(),
  offset: z.number().int().nonnegative().optional(),
});

const requeueInput = z.object({ jobId: z.string().min(1) });

export const jobsRouter = router({
  list: publicProcedure.input(listInput).query(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.listJobs(input);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),

  requeue: publicProcedure.input(requeueInput).mutation(async ({ ctx, input }) => {
    try {
      return await ctx.gateway.requeueJob(input.jobId);
    } catch (error) {
      rethrowGatewayError(error);
    }
  }),
});
