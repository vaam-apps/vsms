"use client";

import { createTRPCReact } from "@trpc/react-query";
// Type-only, per this package's own contract (see index.ts's module doc
// and the root architecture plan §2): a value import here would drag
// `@vsms/api`'s server-only module graph — `node:fs` reads for cert/key
// material included — into the browser bundle. `verbatimModuleSyntax`
// (tsconfig.base.json) makes accidentally dropping the `type` keyword a
// compile error, not a silent bundle-size regression.
import type { AppRouter } from "@vsms/api";

export const trpc = createTRPCReact<AppRouter>();
