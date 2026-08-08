import "server-only";

import { composeRouter } from "./routers/compose";
import { messagesRouter } from "./routers/messages";
import { router } from "./trpc";

export const appRouter = router({
  compose: composeRouter,
  messages: messagesRouter,
});

export type AppRouter = typeof appRouter;
