// Pure module (R6): the "Record an opt-out" form's validation, extracted
// so it has a home outside the screen and a test beside it
// (`record-opt-out-schema.test.ts`). Mirrors `providers-screen.tsx`'s own
// `editSchema` shape (`z.string().trim().min(1, "...")` for a required
// field) — nothing new invented for this form.

import { z } from "zod";

export const OPT_OUT_SOURCES = ["inbound_stop", "admin", "import", "operator"] as const;

export const recordOptOutSchema = z.object({
  msisdn: z.string().trim().min(1, "MSISDN is required"),
  source: z.enum(OPT_OUT_SOURCES),
  scope: z.string().trim().min(1, "Scope is required"),
  reason: z.string().trim(),
});

export type RecordOptOutFormValues = z.infer<typeof recordOptOutSchema>;

export const RECORD_OPT_OUT_DEFAULTS: RecordOptOutFormValues = {
  msisdn: "",
  source: "admin",
  scope: "all",
  reason: "",
};
