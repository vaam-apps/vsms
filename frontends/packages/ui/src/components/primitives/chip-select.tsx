"use client";

// No `CheckboxGroup` — Headless UI 2.2.10 does not export one (checked,
// not assumed: `tsc` rejected it), so the grouping is done here.
//
// A `<fieldset>`, not `<div role="group">`. Biome's `a11y/useSemanticElements`
// is right that the native element beats the ARIA role, and it caught the
// div in CI after a local `biome check frontends` had passed — CI runs
// `pnpm biome ci .`, which is not the same command. Run CI's own.
//
// `min-w-0` because a `<fieldset>` has a UA `min-width: min-content` that a
// `<div>` does not, which would otherwise stop the chips wrapping inside a
// narrow drawer.
import { Checkbox, Field, Label } from "@headlessui/react";
import { Check } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * Multi-select over a small, fixed vocabulary, rendered as toggleable
 * chips.
 *
 * # Why chips rather than a text field or a multi-select
 *
 * The motivating case is OAuth scopes when provisioning a client. That
 * field was a free-text input: an operator had to already know both that
 * scopes are space-delimited *and* what the fourteen valid strings are,
 * with a typo silently producing a client that is denied at Layer 2 with
 * no hint why. Showing the vocabulary is the whole point.
 *
 * Not a `Select` with `multiple`: this renders inline with no portal and
 * no transition, so it cannot hit the focus-trap bug that made `Select`
 * unusable inside a drawer (#315). Same reasoning as `RadioGroup` — for a
 * bounded vocabulary in a drawer, the inline control is the safer one.
 *
 * Not a `<datalist>` or a tag input either: both still let a caller type
 * something outside the vocabulary, which is exactly the property being
 * removed.
 */
export interface ChipOption<T extends string> {
  value: T;
  label: ReactNode;
  /** Shown beneath the label — for scopes, what the scope actually permits. */
  description?: ReactNode;
}

export interface ChipSelectProps<T extends string> {
  value: readonly T[];
  onValueChange: (value: T[]) => void;
  options: readonly ChipOption<T>[];
  disabled?: boolean | undefined;
  "aria-label"?: string | undefined;
  className?: string | undefined;
}

export function ChipSelect<T extends string>({
  value,
  onValueChange,
  options,
  disabled,
  className,
  ...aria
}: ChipSelectProps<T>) {
  return (
    <fieldset className={cn("flex min-w-0 flex-wrap gap-2", className)} {...aria}>
      {options.map((option) => {
        const checked = value.includes(option.value);
        return (
          <Field key={option.value}>
            <Checkbox
              checked={checked}
              disabled={disabled ?? false}
              onChange={(next: boolean) => {
                onValueChange(
                  next ? [...value, option.value] : value.filter((v) => v !== option.value),
                );
              }}
              className={cn(
                "flex cursor-pointer items-start gap-2 rounded-sm border border-edge bg-surface-2 px-3 py-2 text-body text-muted-foreground",
                "data-checked:border-state-success-border data-checked:bg-state-success-bg data-checked:text-state-success-fg",
                "data-focus:outline-none data-focus:ring-1 data-focus:ring-state-success-border",
                "data-disabled:cursor-not-allowed data-disabled:opacity-50",
              )}
            >
              <span
                className={cn(
                  "mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-xs border border-edge",
                  checked && "border-state-success-fg",
                )}
              >
                {checked && <Check className="size-3" aria-hidden="true" />}
              </span>
              <span className="flex flex-col gap-0.5">
                <Label className="cursor-pointer font-mono font-medium">{option.label}</Label>
                {option.description !== undefined && (
                  <span className="text-caption text-subtle-foreground">{option.description}</span>
                )}
              </span>
            </Checkbox>
          </Field>
        );
      })}
    </fieldset>
  );
}
