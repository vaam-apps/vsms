// Shared wire types for the Users & roles screen and its dumb view
// components.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type UserListItem = RouterOutputs["users"]["list"][number];
export type UserDetail = NonNullable<RouterOutputs["users"]["get"]>["data"];
export type RoleRecord = RouterOutputs["roles"]["list"][number];
export type RoleDetail = NonNullable<RouterOutputs["roles"]["get"]>["data"];

export const TABS = ["users", "roles"] as const;
export type UsersRolesTab = (typeof TABS)[number];
