// Type-only — see `composer-screen.tsx`'s own note. `admin` already
// depends on `@vsms/api` for its route handler; this is a second, purely
// type-level use of that same dependency, erased at build time
// (`verbatimModuleSyntax`), not a new runtime import of the server router.

import type { inferRouterOutputs } from "@trpc/server";
import type { AppRouter } from "@vsms/api";

type RouterOutputs = inferRouterOutputs<AppRouter>;

export type ComposeSendResult = RouterOutputs["compose"]["send"];
