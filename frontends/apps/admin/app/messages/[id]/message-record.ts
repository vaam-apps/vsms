// Type-only shapes shared by `message-detail-screen.tsx` and its dumb
// components below — derived once here rather than re-derived per file.
// Not imported from `@vsms/gateway` directly: every consumer only ever
// needs what the router itself infers, via `@trpc/server`'s
// `inferRouterOutputs`, type-only throughout (erased at build time,
// `verbatimModuleSyntax`) — not a runtime import of the server router.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type MessageDetail = RouterOutputs["messages"]["byId"];
export type DeliveryReceiptSummary = RouterOutputs["messages"]["receipts"]["receipts"][number];
