import "server-only";

import { composeRouter } from "./routers/compose";
import { dashboardRouter } from "./routers/dashboard";
import { jobsRouter } from "./routers/jobs";
import { messagesRouter } from "./routers/messages";
import { providersRouter } from "./routers/providers";
import { routeSimulatorRouter } from "./routers/route-simulator";
import { routesRouter } from "./routers/routes";
import { workersRouter } from "./routers/workers";
import { router } from "./trpc";

export const appRouter = router({
  compose: composeRouter,
  dashboard: dashboardRouter,
  messages: messagesRouter,
  jobs: jobsRouter,
  workers: workersRouter,
  providers: providersRouter,
  routes: routesRouter,
  routeSimulator: routeSimulatorRouter,
});

export type AppRouter = typeof appRouter;
