"use client";

import { createTRPCReact } from "@trpc/react-query";
// Type-only: the React bindings need the router's *type* and nothing else.
// The router itself is a value only `createLocalFetch` (`./local-fetch.ts`)
// takes, handed in by the console. `verbatimModuleSyntax`
// (tsconfig.base.json) makes accidentally dropping the `type` keyword a
// compile error.
import type { AppRouter } from "@vsms/api";

export const trpc = createTRPCReact<AppRouter>();
