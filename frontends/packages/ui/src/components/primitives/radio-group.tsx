"use client";

import { RadioGroup as HeadlessRadioGroup, Radio } from "@headlessui/react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * A single-choice control over a small, fixed vocabulary.
 *
 * # Why this exists rather than reaching for `Select`
 *
 * Two reasons, and the second is a correctness one.
 *
 * The first is the one that prompted it: for a handful of options, a
 * `Select` makes the reader click once to discover what the choices even
 * are, and again to pick. A radio group shows the whole vocabulary and
 * costs one click. `SenderIdRegistrationStatus` has four values and lives
 * in a review drawer where seeing the other three *is* the decision.
 *
 * The second: this cannot hit the bug that made `Select` unusable inside a
 * drawer for its entire existence (#315). Headless UI's `Listbox` portals
 * its options to a top-level sibling of `<body>`, which lands outside
 * vaul's Radix focus trap and stalls the enter transition —
 * `opacity: 0`, `pointer-events: none`, measured live. `RadioGroup`
 * renders inline with no portal and no transition, so there is no
 * equivalent failure available to it. For a small vocabulary inside a
 * drawer, that makes radio the *safer* control, not merely the friendlier
 * one.
 *
 * Deliberately not built on `Select`'s CVA table: there are no visual
 * variants to parameterise, and every option renders identically apart
 * from its checked state.
 */
export interface RadioGroupOption<T extends string> {
  value: T;
  label: ReactNode;
  /** Optional one-line explanation rendered under the label. */
  description?: ReactNode;
}

export interface RadioGroupProps<T extends string> {
  value: T | undefined;
  onValueChange: (value: T) => void;
  options: readonly RadioGroupOption<T>[];
  /** Accessible name. Pair with `FormField`'s label via `aria-labelledby`
   * where one exists; supplied directly when the group stands alone. */
  "aria-label"?: string | undefined;
  "aria-labelledby"?: string | undefined;
  disabled?: boolean | undefined;
  className?: string | undefined;
}

export function RadioGroup<T extends string>({
  value,
  onValueChange,
  options,
  disabled,
  className,
  ...aria
}: RadioGroupProps<T>) {
  return (
    <HeadlessRadioGroup
      value={value ?? null}
      onChange={(next: T | null) => {
        if (next !== null) onValueChange(next);
      }}
      disabled={disabled ?? false}
      className={cn("flex flex-wrap gap-2", className)}
      {...aria}
    >
      {options.map((option) => (
        <Radio
          key={option.value}
          value={option.value}
          className={cn(
            "flex cursor-pointer flex-col gap-0.5 rounded-sm border border-edge bg-surface-2 px-3 py-2 text-body text-muted-foreground",
            "data-checked:border-state-success-border data-checked:bg-state-success-bg data-checked:text-state-success-fg",
            "data-focus:outline-none data-focus:ring-1 data-focus:ring-state-success-border",
            "data-disabled:cursor-not-allowed data-disabled:opacity-50",
          )}
        >
          <span className="font-medium">{option.label}</span>
          {option.description !== undefined && (
            <span className="text-caption text-subtle-foreground">{option.description}</span>
          )}
        </Radio>
      ))}
    </HeadlessRadioGroup>
  );
}
