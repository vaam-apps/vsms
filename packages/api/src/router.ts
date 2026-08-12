import "server-only";

import { composeRouter } from "./routers/compose";
import { jobsRouter } from "./routers/jobs";
import { messagesRouter } from "./routers/messages";
import { workersRouter } from "./routers/workers";
import { router } from "./trpc";

export const appRouter = router({
  compose: composeRouter,
  messages: messagesRouter,
  jobs: jobsRouter,
  workers: workersRouter,
});

export type AppRouter = typeof appRouter;
