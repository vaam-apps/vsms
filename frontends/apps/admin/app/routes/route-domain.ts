// Pure, React-free domain logic for the Routes screen (#54, R6): the form
// schema, its empty-state defaults, and the predicate-summary formatter.
// Extracted verbatim out of routes-screen.tsx so it can be unit-tested
// without mounting React and so the smart screen component reads as fetch +
// handlers + composition, per AGENTS.md's R6.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";
import { z } from "zod";

type RouterOutputs = inferRouterOutputs<AppRouter>;
export type RouteListItem = RouterOutputs["routes"]["list"][number];
export type ProviderListItem = RouterOutputs["providers"]["list"][number];

// Not shared with `MessageClass` (`@vsms/ui`) — no second screen currently
// needs this specific list, unlike `MESSAGE_CLASSES`.
export const OPERATOR_CODES = ["mtn", "orange", "camtel", "nexttel", "unknown"] as const;
export type OperatorCode = (typeof OPERATOR_CODES)[number];

/** Sentinel for "no predicate set" in the `Select`-backed form fields below
 * — `Select` cannot represent an empty string as a distinct option, and
 * `NULL` on the wire already means "matches anything" (§6.3). */
export const ANY_PREDICATE = "__any";

export function predicateSummary(route: {
  matchOperator?: string | undefined;
  matchClass?: string | undefined;
  matchAppId?: string | undefined;
  matchPrefix?: string | undefined;
}): string {
  const parts: string[] = [];
  if (route.matchOperator !== undefined) parts.push(`operator=${route.matchOperator}`);
  if (route.matchClass !== undefined) parts.push(`class=${route.matchClass}`);
  if (route.matchAppId !== undefined) parts.push("app-scoped");
  if (route.matchPrefix !== undefined) parts.push(`prefix=${route.matchPrefix}`);
  return parts.length === 0 ? "matches anything" : parts.join(", ");
}

export const routeSchema = z.object({
  name: z.string().trim().min(1, "Name is required"),
  priority: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 0 && Number(v) <= 1000, {
      message: "0–1000",
    }),
  weight: z
    .string()
    .trim()
    .refine((v) => Number.isInteger(Number(v)) && Number(v) >= 0 && Number(v) <= 1000, {
      message: "0–1000",
    }),
  enabled: z.enum(["enabled", "disabled"]),
  matchOperator: z.string(),
  matchClass: z.string(),
  matchAppId: z.string(),
  matchPrefix: z.string(),
  providerId: z.string().min(1, "Select a provider"),
});
export type RouteFormValues = z.infer<typeof routeSchema>;

export const EMPTY_ROUTE_FORM_VALUES: RouteFormValues = {
  name: "",
  priority: "0",
  weight: "1",
  enabled: "enabled",
  matchOperator: ANY_PREDICATE,
  matchClass: ANY_PREDICATE,
  matchAppId: "",
  matchPrefix: "",
  providerId: "",
};
