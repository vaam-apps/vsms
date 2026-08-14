// Shared wire types for the Apps screen and its dumb view components.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type AppListItem = RouterOutputs["apps"]["list"][number];
export type AppDetail = NonNullable<RouterOutputs["apps"]["get"]>["data"];
export type AppClientListItem = RouterOutputs["appClients"]["listForApp"][number];
