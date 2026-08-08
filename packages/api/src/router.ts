import "server-only";

import { composeRouter } from "./routers/compose";
import { router } from "./trpc";

export const appRouter = router({
  compose: composeRouter,
});

export type AppRouter = typeof appRouter;
