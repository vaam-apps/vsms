// Dumb component (R6): the more-detail drawer and its edit form. Markup
// moved verbatim out of `providers-screen.tsx`. Takes the `react-hook-form`
// control/register/errors it's handed rather than owning the form itself —
// validation rules (`edit-schema.ts`) and the submit handler both stay with
// the smart component; this file only renders inputs bound to what it's
// given and reports the one event (`onSubmit`) it can't originate itself.

import {
  Button,
  FormField,
  IdDisplay,
  InlineBanner,
  Input,
  MoreDetailDrawer,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Skeleton,
} from "@vsms/ui";
import type { FieldErrors, UseFormRegister } from "react-hook-form";
import { type Control, Controller } from "react-hook-form";
import type { EditFormValues } from "../edit-schema";
import { PROVIDER_STATES } from "../provider-types";

const FORM_ID = "provider-edit-form";

export interface EditableProviderDetail {
  key: string;
  kind: string;
  credentialRef: string;
  displayName: string;
}

export interface ProviderEditDrawerProps {
  open: boolean;
  /** The record id being edited — shown in the header regardless of
   * whether `detail` has finished loading yet, matching the original
   * screen's own `panelId !== null` check. */
  recordId: string | null;
  onClose: () => void;
  isLoadingDetail: boolean;
  detail: EditableProviderDetail | undefined;
  control: Control<EditFormValues>;
  register: UseFormRegister<EditFormValues>;
  errors: FieldErrors<EditFormValues>;
  onSubmit: (event: React.FormEvent<HTMLFormElement>) => void;
  isSaving: boolean;
  canSave: boolean;
  saveError: string | null;
}

export function ProviderEditDrawer({
  open,
  recordId,
  onClose,
  isLoadingDetail,
  detail,
  control,
  register,
  errors,
  onSubmit,
  isSaving,
  canSave,
  saveError,
}: ProviderEditDrawerProps) {
  return (
    <MoreDetailDrawer
      open={open}
      onOpenChange={(nextOpen) => !nextOpen && onClose()}
      title={detail?.displayName ?? "Provider"}
      description={recordId !== null && <IdDisplay value={recordId} variant="full" />}
      footer={
        <>
          <Button type="button" variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button type="submit" form={FORM_ID} disabled={isSaving || !canSave}>
            {isSaving ? "Saving…" : "Save"}
          </Button>
        </>
      }
    >
      {isLoadingDetail && <Skeleton className="h-32 w-full" />}

      {detail !== undefined && (
        <form id={FORM_ID} onSubmit={onSubmit} className="flex flex-col gap-4">
          <div className="grid grid-cols-2 gap-3 rounded-sm border border-edge bg-surface-2 p-3 text-caption text-muted-foreground">
            <div>
              Key: <span className="font-mono text-foreground">{detail.key}</span>
            </div>
            <div>
              Kind: <span className="font-mono text-foreground">{detail.kind}</span>
            </div>
            <div className="col-span-2">
              Credential ref:{" "}
              <span className="font-mono text-foreground">{detail.credentialRef}</span>
            </div>
            <div className="col-span-2 text-subtle-foreground">
              Key/kind/config/credential ref are infrastructure wiring, set once at provisioning —
              not editable from this form.
            </div>
          </div>

          <FormField
            label="Display name"
            htmlFor="provider-display-name"
            error={errors.displayName?.message}
          >
            <Input
              id="provider-display-name"
              aria-invalid={errors.displayName != null}
              {...register("displayName")}
            />
          </FormField>

          <FormField label="State" htmlFor="provider-state">
            <Controller
              control={control}
              name="state"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="provider-state">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROVIDER_STATES.map((state) => (
                      <SelectItem key={state} value={state}>
                        {state}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </FormField>

          <div className="grid grid-cols-2 gap-3">
            <FormField label="Max TPS" htmlFor="provider-max-tps" error={errors.maxTps?.message}>
              <Input
                id="provider-max-tps"
                inputMode="decimal"
                aria-invalid={errors.maxTps != null}
                {...register("maxTps")}
              />
            </FormField>
            <FormField
              label="Max daily submissions"
              htmlFor="provider-max-daily"
              error={errors.maxDailySubmissions?.message}
            >
              <Input
                id="provider-max-daily"
                inputMode="numeric"
                aria-invalid={errors.maxDailySubmissions != null}
                {...register("maxDailySubmissions")}
              />
            </FormField>
          </div>

          <FormField
            label="Cost per segment (XAF)"
            htmlFor="provider-cost"
            error={errors.costPerSegmentXaf?.message}
          >
            <Input
              id="provider-cost"
              aria-invalid={errors.costPerSegmentXaf != null}
              {...register("costPerSegmentXaf")}
            />
          </FormField>

          {saveError != null && (
            <InlineBanner variant="danger">Save failed: {saveError}</InlineBanner>
          )}
        </form>
      )}
    </MoreDetailDrawer>
  );
}
