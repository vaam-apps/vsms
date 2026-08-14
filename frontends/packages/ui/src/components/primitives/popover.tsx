"use client";

import { Popover as HeadlessPopover, PopoverButton, PopoverPanel } from "@headlessui/react";
import type { ComponentPropsWithoutRef } from "react";
import { cn } from "../../lib/cn";

// D3: Radix `Popover` → Headless UI `Popover`/`PopoverButton`/`PopoverPanel`.
// `Popover`/`PopoverButton` are used as-is — Headless UI's own `Popover` is
// already a fully self-contained open/close state machine (no separate
// "Root" plumbing needed the way `Dialog` needs one here, since Popover
// natively supports an in-place, uncontrolled trigger).
export const Popover = HeadlessPopover;

// `PopoverButton` is natively polymorphic (`as`), replacing Radix's
// `asChild` pattern (D4's same call, applied here to a trigger rather than
// `Button` itself): `<PopoverTrigger as={Button} variant="secondary">…`.
export const PopoverTrigger = PopoverButton;

// Radix also exported a standalone `Anchor` (a positioning reference
// distinct from the trigger) — unconsumed anywhere in this codebase, and
// Headless UI's own `anchor` prop on `PopoverPanel` (below) covers the same
// need without a separate element, so no replacement is exported.

export function PopoverContent({
  className,
  anchor = "bottom start",
  ...props
}: ComponentPropsWithoutRef<typeof PopoverPanel>) {
  return (
    <PopoverPanel
      anchor={anchor}
      transition
      className={cn(
        "z-50 rounded-md border border-edge bg-surface-2 p-3 shadow-[var(--shadow-popover)] [--anchor-gap:6px] focus:outline-none",
        "origin-top transition duration-100 ease-out data-closed:scale-95 data-closed:opacity-0",
        className,
      )}
      {...props}
    />
  );
}
