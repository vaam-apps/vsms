// Shared wire types for the Audit log screen and its dumb view components.
// Kept in one place so a dumb component never has to import from its own
// smart screen file to get a type.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type AuditLogEntry = RouterOutputs["auditLog"]["list"]["entries"][number];
export type ChainStatus = RouterOutputs["auditLog"]["chainStatus"];

export const OPERATIONS = ["create", "update", "delete"] as const;
export type AuditOperation = (typeof OPERATIONS)[number];
