"use client";

import { Field, Label as HeadlessLabel } from "@headlessui/react";
import { Fragment, forwardRef } from "react";
import { cn } from "../../lib/cn";
import { omitUndefined } from "../../lib/omit-undefined";

/**
 * D3: ports from Radix `Label` to Headless UI's `Field`/`Label` composition.
 *
 * **Found live, not assumed:** Headless UI's own `Label` throws ("You used a
 * `<Label />` component, but it is not inside a relevant parent") unless a
 * `Field` ancestor exists to supply its context — unlike Radix's `Label`,
 * which was a fully standalone `<label>` wrapper. None of this console's
 * ~15 call sites wrap their `<Label htmlFor="x">…</Label>` +
 * `<Input id="x">` pair in a `Field` (they associate the two explicitly via
 * matching `htmlFor`/`id` instead, which needs no shared context at all),
 * so porting to the bare Headless UI `Label` export would crash every one
 * of them. Wrapping each call site in `<Field>` was considered and
 * rejected — it would touch ~15 files outside this bucket's scope for no
 * behavioural gain, since every call site already supplies its own
 * `htmlFor`.
 *
 * Fixed here instead: `Label` supplies its own self-contained `Field`
 * (`as={Fragment}`, so it adds no DOM node) around just the `Label` itself.
 * This satisfies Headless UI's context requirement without requiring any
 * caller to know `Field` exists, keeps the public API
 * (`<Label htmlFor="x">…</Label>`) byte-identical to the Radix version, and
 * still genuinely uses Headless UI's `Label` (constraint 6) rather than a
 * hand-rolled `<label>`.
 */
export const Label = forwardRef<HTMLLabelElement, React.ComponentPropsWithoutRef<"label">>(
  ({ className, ...props }, ref) => (
    <Field as={Fragment}>
      <HeadlessLabel
        ref={ref}
        className={cn("text-body font-medium text-foreground", className)}
        {...omitUndefined(props)}
      />
    </Field>
  ),
);
Label.displayName = "Label";
