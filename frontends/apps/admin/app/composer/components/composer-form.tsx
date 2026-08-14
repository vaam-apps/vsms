// Dumb — route-local to the composer (R6). The form's own markup: field
// layout, labels, inline validation messages, the advanced-fields
// disclosure, and the submit button. Takes react-hook-form's `form` object
// as a prop rather than calling `useForm` itself — the hook, the zod
// resolver, and every mutation/query stay in `composer-screen.tsx`; this
// component only renders fields bound to a form it didn't create and
// reports submission upward via `onSubmit`.

import {
  Button,
  EncodingPreview,
  type EncodingPreviewResult,
  FormField,
  InlineBanner,
  Input,
  MESSAGE_CLASSES,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import { type ComposerFormValues, MESSAGE_CLASS_LABELS } from "../composer-schema";

export interface ComposerFormProps {
  form: UseFormReturn<ComposerFormValues>;
  onSubmit: (values: ComposerFormValues) => void;
  encodingPreview: EncodingPreviewResult | null;
  isPreviewLoading: boolean;
  isPreviewStale: boolean;
  onApplySuggestion: () => void;
  showRecipientStatus: boolean;
  normalizedTo: string | undefined;
  recipientOperator: string | undefined;
  generalError: string | null;
  isSending: boolean;
}

export function ComposerForm({
  form,
  onSubmit,
  encodingPreview,
  isPreviewLoading,
  isPreviewStale,
  onApplySuggestion,
  showRecipientStatus,
  normalizedTo,
  recipientOperator,
  generalError,
  isSending,
}: ComposerFormProps) {
  return (
    <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-6">
      <FormField label="Recipient" htmlFor="composer-to" error={form.formState.errors.to?.message}>
        <Input
          id="composer-to"
          placeholder="+237 677 123 456"
          aria-invalid={form.formState.errors.to != null}
          {...form.register("to")}
        />
        {form.formState.errors.to == null && showRecipientStatus && normalizedTo != null && (
          <p className="text-caption text-muted-foreground">
            Normalises to <span className="font-mono text-foreground">{normalizedTo}</span>
            {recipientOperator != null && recipientOperator !== "unknown" && (
              <>
                {" "}
                · <span className="font-mono uppercase">{recipientOperator}</span>
              </>
            )}
          </p>
        )}
      </FormField>

      <FormField
        label="Message"
        htmlFor="composer-body"
        error={form.formState.errors.body?.message}
      >
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
              isLoading={isPreviewLoading}
              isStale={isPreviewStale}
              onApplySuggestion={onApplySuggestion}
            />
          )}
        />
      </FormField>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <FormField
          label="Sender id"
          htmlFor="composer-sender-id"
          error={form.formState.errors.senderId?.message}
        >
          <Input
            id="composer-sender-id"
            placeholder="Default for this app"
            aria-invalid={form.formState.errors.senderId != null}
            {...form.register("senderId")}
          />
        </FormField>

        <FormField label="Class" htmlFor="composer-class">
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
        </FormField>
      </div>

      <details className="rounded-sm border border-edge">
        <summary className="cursor-pointer px-3 py-2 text-body text-foreground">Advanced</summary>
        <div className="flex flex-col gap-4 border-edge border-t p-3">
          <FormField
            label="Client reference"
            htmlFor="composer-client-ref"
            error={form.formState.errors.clientRef?.message}
          >
            <Input
              id="composer-client-ref"
              placeholder="Your own idempotency / correlation id"
              {...form.register("clientRef")}
            />
          </FormField>

          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <FormField label="Scheduled at" htmlFor="composer-scheduled-at">
              <Input
                id="composer-scheduled-at"
                type="datetime-local"
                {...form.register("scheduledAt")}
              />
            </FormField>
            <FormField
              label="Validity (minutes)"
              htmlFor="composer-validity"
              error={form.formState.errors.validityMinutes?.message}
            >
              <Input
                id="composer-validity"
                inputMode="numeric"
                placeholder="Class default"
                aria-invalid={form.formState.errors.validityMinutes != null}
                {...form.register("validityMinutes")}
              />
            </FormField>
          </div>
        </div>
      </details>

      {generalError != null && <InlineBanner variant="danger">{generalError}</InlineBanner>}

      <div className="flex items-center gap-3">
        <Button type="submit" disabled={isSending}>
          {isSending ? "Sending…" : "Send message"}
        </Button>
      </div>
    </form>
  );
}
