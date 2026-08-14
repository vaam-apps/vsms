"use client";

// The composer (#51, T13) — the console's landing screen and its flagship.
// §3.3 of the architecture doc, in one sentence: catch a `ç` before it
// silently doubles the segment count of a send to 50,000 recipients.
//
// `to` and `body` feed a debounced `compose.preview` query as the operator
// types, rendered through `@vsms/ui`'s `EncodingPreview`; submitting calls
// `compose.send`, which triggers a real `sendMessage` against the gateway
// on the console's own machine credential (`SMS_CONSOLE_CLIENT_ID` —
// nothing here proves who the human at the keyboard was, see the
// architecture plan's DECISIONS §1).

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import {
  Button,
  Card,
  CardBody,
  CardHeader,
  EncodingPreview,
  type EncodingPreviewResult,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  StatusPill,
} from "@vsms/ui";
import { useEffect, useState } from "react";
import { Controller, useForm } from "react-hook-form";
import { z } from "zod";

const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;

const MESSAGE_CLASS_LABELS: Record<(typeof MESSAGE_CLASSES)[number], string> = {
  otp: "OTP",
  transactional: "Transactional",
  notification: "Notification",
  marketing: "Marketing",
};

// Mirrors `packages/api/src/routers/compose.ts`'s `sendInput` — read, not
// guessed. `to` and `body` are the only fields `sendInput` actually
// requires; everything else is optional there and stays optional here.
// Client-side bounds mirror the schema's own stored-value constraints
// (`Message.senderIdValue @length(min: 3, max: 11)`, `Message.msisdn
// @length(min: 12, max: 15)`) as a fast-fail UX nicety — `sms-msisdn`'s
// real Cameroon-specific parsing is server-side and is the actual source
// of truth; a value that passes here can still come back 422.
const composerSchema = z.object({
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

type ComposerFormValues = z.infer<typeof composerSchema>;

const DEFAULT_VALUES: ComposerFormValues = {
  to: "",
  body: "",
  senderId: "",
  class: "transactional",
  clientRef: "",
  scheduledAt: "",
  validityMinutes: "",
};

const COMPOSER_FIELDS = [
  "to",
  "body",
  "senderId",
  "class",
  "clientRef",
  "scheduledAt",
  "validityMinutes",
] as const;

function isComposerField(field: string): field is (typeof COMPOSER_FIELDS)[number] {
  return (COMPOSER_FIELDS as readonly string[]).includes(field);
}

/** 250ms per the task brief — long enough that a fast typist doesn't fire
 * a query per keystroke, short enough that the preview still reads as
 * "live." */
function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timeout = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timeout);
  }, [value, delayMs]);
  return debounced;
}

/** Only include `to` in the preview call once it looks like an attempted
 * number, not a fragment of one — `previewMessage` validates a supplied
 * `to` as a real Cameroon mobile (`Msisdn::parse_mobile`) and 422s the
 * *whole call* if it doesn't parse, which would otherwise blank out the
 * encoding stats every keystroke while the operator is still typing the
 * recipient. Once it looks complete, a real invalid number still 422s —
 * `isStale` below is what surfaces that, keeping the last good encoding
 * numbers on screen rather than clearing them. */
function looksLikeAttemptedMsisdn(raw: string): boolean {
  return raw.replace(/\D/g, "").length >= 8;
}

export default function ComposerPage() {
  const form = useForm<ComposerFormValues>({
    resolver: zodResolver(composerSchema),
    defaultValues: DEFAULT_VALUES,
    mode: "onBlur",
  });

  const toValue = form.watch("to");
  const bodyValue = form.watch("body");
  const debouncedTo = useDebouncedValue(toValue, 250);
  const debouncedBody = useDebouncedValue(bodyValue, 250);
  const previewTo = looksLikeAttemptedMsisdn(debouncedTo) ? debouncedTo : undefined;

  const previewQuery = trpc.compose.preview.useQuery(
    { body: debouncedBody, to: previewTo },
    {
      enabled: debouncedBody.trim().length > 0,
      placeholderData: (prev) => prev,
      retry: false,
    },
  );

  const sendMutation = trpc.compose.send.useMutation({
    onSuccess: () => {
      form.reset(DEFAULT_VALUES);
    },
    onError: (error) => {
      const fieldErrors = error.data?.fieldErrors;
      if (fieldErrors == null) return;
      for (const [field, messages] of Object.entries(fieldErrors)) {
        if (isComposerField(field) && messages[0] != null) {
          form.setError(field, { type: "server", message: messages[0] });
        }
      }
    },
  });

  function applySuggestion() {
    const suggestion = previewQuery.data?.suggestion;
    if (suggestion != null) {
      form.setValue("body", suggestion, { shouldDirty: true, shouldTouch: true });
    }
  }

  function onSubmit(values: ComposerFormValues) {
    sendMutation.mutate({
      to: values.to,
      body: values.body,
      senderId: values.senderId === "" ? undefined : values.senderId,
      class: values.class,
      clientRef: values.clientRef === "" ? undefined : values.clientRef,
      scheduledAt:
        values.scheduledAt === "" || values.scheduledAt == null
          ? undefined
          : new Date(values.scheduledAt).toISOString(),
      validityMinutes:
        values.validityMinutes === "" || values.validityMinutes == null
          ? undefined
          : Number(values.validityMinutes),
    });
  }

  // `placeholderData: (prev) => prev` (below) intentionally keeps showing
  // the last successful result across a query-key change — that's what
  // makes an in-flight retype not blank the stats (design doc: "a flicker
  // ... is worse than a stale number"). But it does that unconditionally,
  // including once the body/recipient is genuinely empty again (e.g. right
  // after a successful send resets the form) — without this guard the
  // encoding stats and "Normalises to" line would keep showing the
  // *previous* message's numbers forever, which reads as a bug, not as
  // staleness. Gate on the live input, not on `previewQuery.data`, so an
  // empty field always means an empty preview.
  const encodingPreview: EncodingPreviewResult | null =
    debouncedBody.trim().length > 0 ? (previewQuery.data ?? null) : null;
  const showRecipientStatus = debouncedTo.trim().length > 0 && previewQuery.data != null;
  const hasFieldErrors = sendMutation.error?.data?.fieldErrors != null;
  const generalError = sendMutation.isError && !hasFieldErrors ? sendMutation.error.message : null;

  return (
    <div className="mx-auto flex w-full max-w-[720px] flex-col gap-8">
      <header className="flex flex-col gap-1 border-edge border-b pb-6">
        <h1 className="font-medium text-foreground text-title">Composer</h1>
        <p className="max-w-md text-body text-muted-foreground">
          See exactly what a message will cost before you send it — GSM-7 vs UCS-2, segment count,
          and every character that would force the more expensive encoding.
        </p>
      </header>

      <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-6">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="composer-to">Recipient</Label>
          <Input
            id="composer-to"
            placeholder="+237 677 123 456"
            aria-invalid={form.formState.errors.to != null}
            {...form.register("to")}
          />
          {form.formState.errors.to != null ? (
            <p className="text-caption text-state-danger-fg">{form.formState.errors.to.message}</p>
          ) : (
            showRecipientStatus &&
            previewQuery.data?.normalizedTo != null && (
              <p className="text-caption text-muted-foreground">
                Normalises to{" "}
                <span className="font-mono text-foreground">{previewQuery.data.normalizedTo}</span>
                {previewQuery.data.operator !== "unknown" && (
                  <>
                    {" "}
                    · <span className="font-mono uppercase">{previewQuery.data.operator}</span>
                  </>
                )}
              </p>
            )
          )}
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="composer-body">Message</Label>
          <Controller
            control={form.control}
            name="body"
            render={({ field }) => (
              <EncodingPreview
                id="composer-body"
                name={field.name}
                value={field.value}
                onChange={field.onChange}
                onBlur={field.onBlur}
                aria-invalid={form.formState.errors.body != null}
                preview={encodingPreview}
                isLoading={previewQuery.isFetching}
                isStale={previewQuery.isError}
                onApplySuggestion={applySuggestion}
              />
            )}
          />
          {form.formState.errors.body != null && (
            <p className="text-caption text-state-danger-fg">
              {form.formState.errors.body.message}
            </p>
          )}
        </div>

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="composer-sender-id">Sender id</Label>
            <Input
              id="composer-sender-id"
              placeholder="Default for this app"
              aria-invalid={form.formState.errors.senderId != null}
              {...form.register("senderId")}
            />
            {form.formState.errors.senderId != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.senderId.message}
              </p>
            )}
          </div>

          <div className="flex flex-col gap-1.5">
            <Label htmlFor="composer-class">Class</Label>
            <Controller
              control={form.control}
              name="class"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="composer-class">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {MESSAGE_CLASSES.map((cls) => (
                      <SelectItem key={cls} value={cls}>
                        {MESSAGE_CLASS_LABELS[cls]}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>
        </div>

        <details className="rounded-sm border border-edge">
          <summary className="cursor-pointer px-3 py-2 text-body text-foreground">Advanced</summary>
          <div className="flex flex-col gap-4 border-edge border-t p-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="composer-client-ref">Client reference</Label>
              <Input
                id="composer-client-ref"
                placeholder="Your own idempotency / correlation id"
                {...form.register("clientRef")}
              />
              {form.formState.errors.clientRef != null && (
                <p className="text-caption text-state-danger-fg">
                  {form.formState.errors.clientRef.message}
                </p>
              )}
            </div>

            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="composer-scheduled-at">Scheduled at</Label>
                <Input
                  id="composer-scheduled-at"
                  type="datetime-local"
                  {...form.register("scheduledAt")}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="composer-validity">Validity (minutes)</Label>
                <Input
                  id="composer-validity"
                  inputMode="numeric"
                  placeholder="Class default"
                  aria-invalid={form.formState.errors.validityMinutes != null}
                  {...form.register("validityMinutes")}
                />
                {form.formState.errors.validityMinutes != null && (
                  <p className="text-caption text-state-danger-fg">
                    {form.formState.errors.validityMinutes.message}
                  </p>
                )}
              </div>
            </div>
          </div>
        </details>

        {generalError != null && (
          <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
            {generalError}
          </div>
        )}

        <div className="flex items-center gap-3">
          <Button type="submit" disabled={sendMutation.isPending}>
            {sendMutation.isPending ? "Sending…" : "Send message"}
          </Button>
        </div>
      </form>

      {sendMutation.data != null && (
        <Card>
          <CardHeader title="Message accepted" meta={sendMutation.data.messageId} />
          <CardBody className="flex flex-wrap items-center gap-3">
            <StatusPill state={sendMutation.data.state} showLiteral />
            <span className="font-mono text-caption text-muted-foreground">
              {sendMutation.data.encoding.toUpperCase()} · {sendMutation.data.segments} seg
            </span>
            <span className="font-mono text-caption text-muted-foreground">
              {sendMutation.data.operator === "unknown"
                ? "operator unknown"
                : sendMutation.data.operator.toUpperCase()}
            </span>
            <span className="font-mono text-caption text-muted-foreground">
              ~{sendMutation.data.estimatedCostXaf} FCFA
            </span>
          </CardBody>
        </Card>
      )}
    </div>
  );
}
