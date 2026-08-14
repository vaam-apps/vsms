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
//
// Type-only `@vsms/api` import — see `composer-types.ts`'s own note.

import { zodResolver } from "@hookform/resolvers/zod";
import { trpc } from "@vsms/hooks";
import type { EncodingPreviewResult } from "@vsms/ui";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { ComposerForm } from "./components/composer-form";
import { ComposerHeader } from "./components/composer-header";
import { ComposerLayout } from "./components/composer-layout";
import { ComposerResultCard } from "./components/composer-result-card";
import {
  type ComposerFormValues,
  composerSchema,
  DEFAULT_VALUES,
  isComposerField,
} from "./composer-schema";
import { looksLikeAttemptedMsisdn } from "./composer-validation";

/** 250ms per the task brief — long enough that a fast typist doesn't fire
 * a query per keystroke, short enough that the preview still reads as
 * "live." Not extracted to a pure module: it's a hook (uses `useState`/
 * `useEffect`), not a value that can be unit-tested without React, and
 * it's a "derived value" concern R6 explicitly leaves to the smart
 * component. */
function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timeout = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timeout);
  }, [value, delayMs]);
  return debounced;
}

export function ComposerScreen() {
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

  // `placeholderData: (prev) => prev` (above) intentionally keeps showing
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
    <ComposerLayout>
      <ComposerHeader />
      <ComposerForm
        form={form}
        onSubmit={onSubmit}
        encodingPreview={encodingPreview}
        isPreviewLoading={previewQuery.isFetching}
        isPreviewStale={previewQuery.isError}
        onApplySuggestion={applySuggestion}
        showRecipientStatus={showRecipientStatus}
        normalizedTo={previewQuery.data?.normalizedTo}
        recipientOperator={previewQuery.data?.operator}
        generalError={generalError}
        isSending={sendMutation.isPending}
      />
      {sendMutation.data != null && <ComposerResultCard result={sendMutation.data} />}
    </ComposerLayout>
  );
}
