import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Label } from "./label";

/**
 * A labelled form control with an optional validation error beneath it.
 *
 * This replaces the console's two most-duplicated class strings after the
 * table-column ones:
 *
 * - `"flex flex-col gap-1.5"` — **80 occurrences**, every one of them
 *   wrapping exactly `<Label htmlFor>` + one control (+ sometimes an
 *   error paragraph).
 * - `"text-caption text-state-danger-fg"` — **35 occurrences**, almost all
 *   of them that error paragraph.
 *
 * The shape was identical at every site, checked before this component was
 * designed rather than assumed:
 *
 * ```tsx
 * <div className="flex flex-col gap-1.5">
 *   <Label htmlFor="record-msisdn">MSISDN</Label>
 *   <Input id="record-msisdn" {...form.register("msisdn")} />
 *   <p className="text-caption text-state-danger-fg">{errors.msisdn.message}</p>
 * </div>
 * ```
 *
 * becomes
 *
 * ```tsx
 * <FormField label="MSISDN" htmlFor="record-msisdn" error={errors.msisdn?.message}>
 *   <Input id="record-msisdn" {...form.register("msisdn")} />
 * </FormField>
 * ```
 *
 * **Why a semantic component and not a generic `<Stack gap="1.5">`.** A
 * layout primitive parameterised by its own CSS is a `<div>` with extra
 * steps — it moves the class from the call site into a prop at the call
 * site, which is not factorisation. `FormField` instead names the *thing*
 * (a labelled control with an error slot), so the spacing, the label
 * treatment and the error treatment are one decision in one place, and a
 * caller cannot get the error styling wrong by writing the `<p>` itself.
 *
 * `htmlFor` is required rather than optional. Several of the 80 sites had
 * a `<Label>` with no `htmlFor` at all, which silently breaks
 * click-to-focus and screen-reader association; making it required turns
 * that into a compile error instead of an accessibility bug nobody
 * notices.
 */
export interface FormFieldProps {
  label: ReactNode;
  /** The `id` of the control this labels. Required — see the module doc. */
  htmlFor: string;
  /** Validation message. `undefined` renders no error element at all. */
  error?: string | undefined;
  /** Help text rendered between the label and the control. */
  hint?: ReactNode;
  children: ReactNode;
  className?: string | undefined;
}

export function FormField({ label, htmlFor, error, hint, children, className }: FormFieldProps) {
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      <Label htmlFor={htmlFor}>{label}</Label>
      {hint !== undefined && <p className="text-caption text-muted-foreground">{hint}</p>}
      {children}
      {error !== undefined && <FieldError>{error}</FieldError>}
    </div>
  );
}

/**
 * A validation message.
 *
 * Exported separately because a handful of the 35 sites are not inside a
 * `FormField` — a form-level error above the submit button, or an error
 * attached to a control group rather than one input. Those should still
 * use the same treatment rather than re-inlining the class.
 *
 * `role="alert"` so the message is announced when it appears, which none
 * of the 35 hand-rolled `<p>` elements did.
 */
export function FieldError({
  children,
  className,
}: {
  children: ReactNode;
  className?: string | undefined;
}) {
  return (
    <p role="alert" className={cn("text-caption text-state-danger-fg", className)}>
      {children}
    </p>
  );
}
