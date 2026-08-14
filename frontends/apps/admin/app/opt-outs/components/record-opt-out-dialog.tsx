// Dumb component (R6): the "Record an opt-out" form, moved out of
// `opt-outs-screen.tsx`'s `RecordDialog`. Two changes from the original,
// both R6-required, neither a rewrite of the form's own behaviour:
//
// 1. It no longer calls `trpc.optOuts.record.useMutation` itself — the
//    smart screen owns the mutation and passes `onSubmit`/`isPending`/
//    `errorMessage` down. The pre-R6 version had this same "dumb component
//    fetches its own data" shape twice in this file (`SearchPanel` and
//    this one); only `SearchPanel` had been flagged.
// 2. Field state moved from four `useState` calls to `react-hook-form` +
//    `zod` (validated: MSISDN and scope both required; reason optional).
//    `form` is owned by the smart screen (`useForm` needs a resolver and a
//    reset-on-success call the mutation's `onSuccess` triggers) and handed
//    down whole, the same way `providers-screen.tsx` already threads
//    `form.register`/`Controller` through JSX — nothing new invented here,
//    just relocated behind the smart/dumb boundary.

import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@vsms/ui";
import { Controller, type UseFormReturn } from "react-hook-form";
import { OPT_OUT_SOURCES, type RecordOptOutFormValues } from "../record-opt-out-schema";

export interface RecordOptOutDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  form: UseFormReturn<RecordOptOutFormValues>;
  onSubmit: (values: RecordOptOutFormValues) => void;
  isPending: boolean;
  errorMessage?: string | undefined;
}

export function RecordOptOutDialog({
  open,
  onOpenChange,
  form,
  onSubmit,
  isPending,
  errorMessage,
}: RecordOptOutDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[480px]">
        <DialogHeader>
          <DialogTitle>Record an opt-out</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-msisdn">MSISDN</Label>
            <Input
              id="record-msisdn"
              placeholder="+237677123456"
              aria-invalid={form.formState.errors.msisdn != null}
              {...form.register("msisdn")}
            />
            {form.formState.errors.msisdn != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.msisdn.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-source">Source</Label>
            <Controller
              control={form.control}
              name="source"
              render={({ field }) => (
                <Select value={field.value} onValueChange={field.onChange}>
                  <SelectTrigger id="record-source">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {OPT_OUT_SOURCES.map((s) => (
                      <SelectItem key={s} value={s}>
                        {s}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-scope">Scope</Label>
            <Input
              id="record-scope"
              aria-invalid={form.formState.errors.scope != null}
              {...form.register("scope")}
            />
            {form.formState.errors.scope != null && (
              <p className="text-caption text-state-danger-fg">
                {form.formState.errors.scope.message}
              </p>
            )}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="record-reason">Reason (optional)</Label>
            <Input id="record-reason" {...form.register("reason")} />
          </div>
          {errorMessage != null && (
            <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
              {errorMessage}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="button" disabled={isPending} onClick={form.handleSubmit(onSubmit)}>
            {isPending ? "Recording…" : "Record"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
