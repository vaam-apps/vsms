// The composer's form shape (R6, AGENTS.md) — extracted verbatim from the
// root `page.tsx`/`composer-screen.tsx`. Mirrors
// `frontends/packages/api/src/routers/compose.ts`'s `sendInput` — read,
// not guessed. `to` and `body` are the only fields `sendInput` actually
// requires; everything else is optional there and stays optional here.
// Client-side bounds mirror the schema's own stored-value constraints
// (`Message.senderIdValue @length(min: 3, max: 11)`, `Message.msisdn
// @length(min: 12, max: 15)`) as a fast-fail UX nicety — `sms-msisdn`'s
// real Cameroon-specific parsing is server-side and is the actual source
// of truth; a value that passes here can still come back 422.

import { MESSAGE_CLASSES } from "@vsms/ui";
import { z } from "zod";

export const MESSAGE_CLASS_LABELS: Record<(typeof MESSAGE_CLASSES)[number], string> = {
  otp: "OTP",
  transactional: "Transactional",
  notification: "Notification",
  marketing: "Marketing",
};

export const composerSchema = z.object({
  to: z
    .string()
    .trim()
    .min(1, "Enter a recipient number")
    .max(20, "That's too long for a phone number")
    .regex(/^[+0-9 ]+$/, "Digits, spaces and a leading + only"),
  body: z.string().trim().min(1, "Message body is required"),
  senderId: z
    .string()
    .trim()
    .max(11, "Sender ids are 3–11 characters")
    .refine((v) => v === "" || v.length >= 3, "Sender ids are 3–11 characters")
    .optional(),
  class: z.enum(MESSAGE_CLASSES),
  clientRef: z.string().trim().max(120, "Keep it under 120 characters").optional(),
  scheduledAt: z.string().optional(),
  validityMinutes: z
    .string()
    .trim()
    .refine((v) => v === "" || /^\d+$/.test(v), "Whole minutes only")
    .optional(),
});

export type ComposerFormValues = z.infer<typeof composerSchema>;

export const DEFAULT_VALUES: ComposerFormValues = {
  to: "",
  body: "",
  senderId: "",
  class: "transactional",
  clientRef: "",
  scheduledAt: "",
  validityMinutes: "",
};

export const COMPOSER_FIELDS = [
  "to",
  "body",
  "senderId",
  "class",
  "clientRef",
  "scheduledAt",
  "validityMinutes",
] as const;

export function isComposerField(field: string): field is (typeof COMPOSER_FIELDS)[number] {
  return (COMPOSER_FIELDS as readonly string[]).includes(field);
}
