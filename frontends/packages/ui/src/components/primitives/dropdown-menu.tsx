"use client";

import {
  Menu,
  MenuButton,
  MenuItem,
  MenuItems,
  MenuSection,
  MenuSeparator,
} from "@headlessui/react";
import { Check } from "lucide-react";
import type { ComponentPropsWithoutRef } from "react";
import { cn } from "../../lib/cn";
import { omitUndefined } from "../../lib/omit-undefined";

// D3: Radix `DropdownMenu` → Headless UI `Menu`/`MenuButton`/`MenuItems`/`MenuItem`.
export const DropdownMenu = Menu;

// `MenuButton` is natively polymorphic (`as`), replacing Radix's `asChild`
// pattern (D4's same call, applied to a trigger rather than `Button`
// itself): `<DropdownMenuTrigger as={Button} variant="secondary">…`.
export const DropdownMenuTrigger = MenuButton;

// `DropdownMenuGroup`/`DropdownMenuCheckboxItem` have zero call sites in
// this codebase today (grepped across `admin/` and `packages/ui`) — kept
// for API parity rather than dropped silently, mapped onto Headless UI's
// closest equivalents.
export const DropdownMenuGroup = MenuSection;

export function DropdownMenuSeparator({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof MenuSeparator>) {
  return <MenuSeparator className={cn("my-1 h-px bg-edge", className)} {...props} />;
}

export function DropdownMenuContent({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof MenuItems>) {
  return (
    <MenuItems
      anchor="bottom start"
      transition
      className={cn(
        "z-50 min-w-[180px] rounded-md border border-edge bg-surface-2 p-1 shadow-[var(--shadow-popover)] [--anchor-gap:4px] focus:outline-none",
        "origin-top transition duration-100 ease-out data-closed:scale-95 data-closed:opacity-0",
        className,
      )}
      {...props}
    />
  );
}

export function DropdownMenuItem({
  className,
  ...props
}: Omit<ComponentPropsWithoutRef<"button">, "type">) {
  return (
    <MenuItem
      as="button"
      type="button"
      className={cn(
        "flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left text-body text-foreground outline-none",
        "data-focus:bg-surface-3",
        "data-disabled:pointer-events-none data-disabled:opacity-50",
        className,
      )}
      {...omitUndefined(props)}
    />
  );
}

/** Unconsumed today (see module doc above) — caller controls `checked`
 * itself, since Headless UI's `Menu` has no built-in checkbox-item state
 * the way Radix's `DropdownMenuCheckboxItem` did. */
export function DropdownMenuCheckboxItem({
  className,
  checked = false,
  children,
  ...props
}: Omit<ComponentPropsWithoutRef<"button">, "type"> & { checked?: boolean }) {
  return (
    <MenuItem
      as="button"
      type="button"
      role="menuitemcheckbox"
      aria-checked={checked}
      className={cn(
        "flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left text-body text-foreground outline-none",
        "data-focus:bg-surface-3",
        className,
      )}
      {...omitUndefined(props)}
    >
      <span className="flex h-3.5 w-3.5 items-center justify-center">
        {checked && <Check size={14} strokeWidth={1.5} aria-hidden="true" />}
      </span>
      {children}
    </MenuItem>
  );
}

/**
 * **Found live, not assumed**: Headless UI's own `MenuHeading` throws the
 * identical "not inside a relevant parent" error `label.tsx`'s own module
 * doc already found for the standalone `Label` — `MenuHeading` needs a
 * `MenuSection` ancestor to supply that context (`MenuSection`'s own
 * implementation calls `useLabels()` to *create* the provider; `MenuHeading`
 * calls `useLabelContext()` to *consume* it), and every call site here
 * (matching the original Radix `DropdownMenuLabel`'s own shape) renders a
 * standalone label with no surrounding group. Radix's own `Label` was a
 * plain, non-interactive `<div>` with no roving-focus/ARIA-grouping
 * behaviour beyond being read as ordinary text inside the menu's
 * accessible tree — so a plain `<div>` here loses nothing real and avoids
 * a dependency this element doesn't actually need. Reproduced live: a real
 * "Uncaught Error: You used a <Label /> component, but it is not inside a
 * relevant parent" client-side exception, thrown the instant the gallery's
 * demo menu opened, before this fix.
 */
export function DropdownMenuLabel({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn("px-2 py-1.5 text-micro text-muted-foreground tracking-[0.03em]", className)}
      {...props}
    />
  );
}
