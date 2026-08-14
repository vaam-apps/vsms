"use client";

import { Command as CommandPrimitive } from "cmdk";
import { Search } from "lucide-react";
import { forwardRef } from "react";
import { cn } from "../../lib/cn";

// cmdk is standalone (no Radix wrapper needed — it already owns its own
// keyboard nav/ARIA). Styled with the same surface-2/border-edge/radius-md
// floating-layer treatment as Popover/DropdownMenu for visual consistency.

export const CommandMenu = forwardRef<
  React.ElementRef<typeof CommandPrimitive>,
  React.ComponentPropsWithoutRef<typeof CommandPrimitive>
>(({ className, ...props }, ref) => (
  <CommandPrimitive
    ref={ref}
    className={cn(
      "flex w-full flex-col overflow-hidden rounded-md border border-edge bg-surface-2 text-foreground",
      className,
    )}
    {...props}
  />
));
CommandMenu.displayName = "CommandMenu";

export function CommandMenuInput(
  props: React.ComponentPropsWithoutRef<typeof CommandPrimitive.Input>,
) {
  return (
    <div className="flex items-center gap-2 border-edge border-b px-3">
      <Search size={14} strokeWidth={1.5} className="shrink-0 text-subtle-foreground" />
      <CommandPrimitive.Input
        className="h-10 w-full bg-transparent text-prose outline-none placeholder:text-subtle-foreground"
        {...props}
      />
    </div>
  );
}

export const CommandMenuList = forwardRef<
  React.ElementRef<typeof CommandPrimitive.List>,
  React.ComponentPropsWithoutRef<typeof CommandPrimitive.List>
>(({ className, ...props }, ref) => (
  <CommandPrimitive.List
    ref={ref}
    className={cn("max-h-80 overflow-y-auto overflow-x-hidden p-1", className)}
    {...props}
  />
));
CommandMenuList.displayName = "CommandMenuList";

export function CommandMenuEmpty(
  props: React.ComponentPropsWithoutRef<typeof CommandPrimitive.Empty>,
) {
  return (
    <CommandPrimitive.Empty
      className="px-3 py-6 text-center text-body text-muted-foreground"
      {...props}
    />
  );
}

export const CommandMenuGroup = forwardRef<
  React.ElementRef<typeof CommandPrimitive.Group>,
  React.ComponentPropsWithoutRef<typeof CommandPrimitive.Group>
>(({ className, ...props }, ref) => (
  <CommandPrimitive.Group
    ref={ref}
    className={cn(
      "text-body [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-micro [&_[cmdk-group-heading]]:text-muted-foreground",
      className,
    )}
    {...props}
  />
));
CommandMenuGroup.displayName = "CommandMenuGroup";

export const CommandMenuItem = forwardRef<
  React.ElementRef<typeof CommandPrimitive.Item>,
  React.ComponentPropsWithoutRef<typeof CommandPrimitive.Item>
>(({ className, ...props }, ref) => (
  <CommandPrimitive.Item
    ref={ref}
    className={cn(
      "flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-body data-[selected=true]:bg-surface-3",
      className,
    )}
    {...props}
  />
));
CommandMenuItem.displayName = "CommandMenuItem";
