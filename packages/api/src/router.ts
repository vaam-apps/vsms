import "server-only";

import { appClientsRouter } from "./routers/app-clients";
import { appsRouter } from "./routers/apps";
import { auditLogRouter } from "./routers/audit-log";
import { composeRouter } from "./routers/compose";
import { dashboardRouter } from "./routers/dashboard";
import { jobsRouter } from "./routers/jobs";
import { messagesRouter } from "./routers/messages";
import { optOutsRouter } from "./routers/opt-outs";
import { providersRouter } from "./routers/providers";
import { rolesRouter } from "./routers/roles";
import { routeSimulatorRouter } from "./routers/route-simulator";
import { routesRouter } from "./routers/routes";
import { usersRouter } from "./routers/users";
import { senderIdRegistrationsRouter, senderIdsRouter } from "./routers/senders";
import { webhookAttemptsRouter, webhookEndpointsRouter } from "./routers/webhooks";
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
  apps: appsRouter,
  appClients: appClientsRouter,
  users: usersRouter,
  roles: rolesRouter,
  optOuts: optOutsRouter,
  auditLog: auditLogRouter,
  senderIds: senderIdsRouter,
  senderIdRegistrations: senderIdRegistrationsRouter,
  webhookEndpoints: webhookEndpointsRouter,
  webhookAttempts: webhookAttemptsRouter,
});

export type AppRouter = typeof appRouter;
