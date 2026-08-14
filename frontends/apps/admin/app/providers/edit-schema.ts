// Pure validation module for the Providers edit form (#54/#211), moved
// verbatim out of `providers-screen.tsx` per AGENTS.md's R6 ("if something
// in it could be unit-tested without React, it does not belong there").
// Mirrors `UpdateProviderFields` (`packages/gateway/src/providers.ts`) — the
// operationally-relevant subset this screen lets a human edit.

import { z } from "zod";
import { PROVIDER_STATES } from "./provider-types";

export const editSchema = z.object({
  displayName: z.string().trim().min(1, "Display name is required"),
  state: z.enum(PROVIDER_STATES),
  maxTps: z
    .string()
    .trim()
    .refine((v) => v !== "" && Number.isFinite(Number(v)) && Number(v) >= 0, "Enter a number ≥ 0"),
  maxDailySubmissions: z
    .string()
    .trim()
    .refine(
      (v) => v !== "" && Number.isInteger(Number(v)) && Number(v) >= 0,
      "Enter a whole number ≥ 0",
    ),
  costPerSegmentXaf: z.string().trim().min(1, "Cost per segment is required"),
});

export type EditFormValues = z.infer<typeof editSchema>;
