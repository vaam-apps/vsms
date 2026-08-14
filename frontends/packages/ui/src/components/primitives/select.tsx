"use client";

import { Listbox, ListboxButton, ListboxOption, ListboxOptions } from "@headlessui/react";
import { Check, ChevronDown } from "lucide-react";
import {
  Children,
  createContext,
  isValidElement,
  type ReactElement,
  type ReactNode,
  useContext,
  useMemo,
  useState,
} from "react";
import { cn } from "../../lib/cn";
import { omitUndefined } from "../../lib/omit-undefined";

/**
 * D17: Radix `Select` → Headless UI `Listbox`. Flagged in the design doc as
 * "the largest single API-shape change in the whole primitives migration" —
 * but Radix `Select`'s own public shape (`Select value/onValueChange` →
 * `SelectTrigger` → `SelectValue` → `SelectContent` → `SelectItem`) is kept
 * byte-identical here, so none of this console's eleven call sites need any
 * change. That's possible because Radix `SelectValue` and Headless UI's
 * `Listbox` solve the "what does the trigger display" problem differently:
 * Radix derives it internally from whichever `SelectItem` matches the
 * current value; `Listbox` has no equivalent (`ListboxButton`'s render prop
 * only exposes the raw `value`, not a matching option's own children/label
 * — e.g. `routes-screen.tsx`'s provider select shows `provider.displayName`
 * for a `value={provider.id}`, not the id itself). `findItemLabel` below
 * replicates Radix's behaviour by walking `Select`'s own `children` tree
 * (not the DOM — `ListboxOptions` may not be mounted while closed) to find
 * the `SelectItem` whose `value` matches, and using *its* children as the
 * label. Keyboard behaviour (type-ahead, `Escape`, arrow-key nav) is
 * genuinely Headless UI's own `Listbox`, not reimplemented here — this file
 * only adds the label-lookup Radix's `SelectValue` used to give for free.
 */

interface SelectContextValue {
  value: string | undefined;
  itemLabel: (value: string) => ReactNode | undefined;
}
const SelectContext = createContext<SelectContextValue | null>(null);

function useSelectContext(component: string): SelectContextValue {
  const ctx = useContext(SelectContext);
  if (ctx === null) {
    throw new Error(`<${component} /> must be rendered inside <Select>.`);
  }
  return ctx;
}

function findItemLabel(node: ReactNode, value: string): ReactNode | undefined {
  let found: ReactNode | undefined;
  Children.forEach(node, (child) => {
    if (found !== undefined || !isValidElement(child)) return;
    const el = child as ReactElement<{ value?: string; children?: ReactNode }>;
    if (el.type === SelectItem) {
      if (el.props.value === value) found = el.props.children;
      return;
    }
    if (el.props?.children != null) {
      found = findItemLabel(el.props.children, value);
    }
  });
  return found;
}

export interface SelectProps {
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
  disabled?: boolean;
  children: ReactNode;
}

export function Select({ value, defaultValue, onValueChange, disabled, children }: SelectProps) {
  const [internalValue, setInternalValue] = useState(defaultValue);
  const isControlled = value !== undefined;
  const currentValue = isControlled ? value : internalValue;

  function handleChange(next: string) {
    if (!isControlled) setInternalValue(next);
    onValueChange?.(next);
  }

  const itemLabel = useMemo(() => (v: string) => findItemLabel(children, v), [children]);

  return (
    <Listbox value={currentValue ?? ""} onChange={handleChange} {...omitUndefined({ disabled })}>
      <SelectContext.Provider value={{ value: currentValue, itemLabel }}>
        {children}
      </SelectContext.Provider>
    </Listbox>
  );
}

// Unconsumed today (grepped across `admin/`) — kept for API parity. Radix's
// `SelectGroup` had no visual treatment of its own beyond semantic
// grouping; Headless UI's `Listbox` has no equivalent, so this stays a
// plain wrapper rather than reaching for `Menu`'s `MenuSection` (a
// different component family).
export function SelectGroup({ children }: { children: ReactNode }) {
  return <fieldset className="contents border-0 p-0 m-0 min-w-0">{children}</fieldset>;
}

export function SelectTrigger({
  id,
  className,
  children,
}: {
  id?: string;
  className?: string;
  children: ReactNode;
}) {
  useSelectContext("SelectTrigger");
  return (
    <ListboxButton
      id={id}
      className={cn(
        "select select-bordered flex w-full items-center justify-between font-sans text-prose",
        className,
      )}
    >
      {children}
      <ChevronDown
        size={14}
        strokeWidth={1.5}
        className="text-muted-foreground"
        aria-hidden="true"
      />
    </ListboxButton>
  );
}

export function SelectValue({ placeholder }: { placeholder?: string }) {
  const { value, itemLabel } = useSelectContext("SelectValue");
  if (value === undefined || value === "") {
    return <span className="text-subtle-foreground">{placeholder}</span>;
  }
  return <>{itemLabel(value) ?? value}</>;
}

export function SelectContent({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  useSelectContext("SelectContent");
  return (
    <ListboxOptions
      anchor="bottom start"
      transition
      className={cn(
        "z-50 max-h-80 min-w-[8rem] overflow-y-auto rounded-md border border-edge bg-surface-2 p-1 shadow-[var(--shadow-popover)] [--anchor-gap:4px] focus:outline-none",
        "origin-top transition duration-100 ease-out data-closed:scale-95 data-closed:opacity-0",
        className,
      )}
    >
      {children}
    </ListboxOptions>
  );
}

export function SelectItem({
  value,
  className,
  children,
}: {
  value: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <ListboxOption
      value={value}
      className={cn(
        "relative flex cursor-pointer items-center rounded-sm py-1.5 pr-2 pl-7 text-body text-foreground outline-none",
        "data-focus:bg-surface-3",
        className,
      )}
    >
      {({ selected }) => (
        <>
          <span className="absolute left-2 flex h-3.5 w-3.5 items-center justify-center">
            {selected && <Check size={14} strokeWidth={1.5} aria-hidden="true" />}
          </span>
          {children}
        </>
      )}
    </ListboxOption>
  );
}
