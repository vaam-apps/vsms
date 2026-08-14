"use client";

import { Tab, TabGroup, TabList, TabPanel, TabPanels } from "@headlessui/react";
import {
  Children,
  Fragment,
  isValidElement,
  type ReactElement,
  type ReactNode,
  useMemo,
  useState,
} from "react";
import { cn } from "../../lib/cn";

/**
 * D18/D3: Radix `Tabs` → Headless UI `TabGroup`, behind a value-based
 * `ValueTabs` adapter. Radix's `Tabs` was value-based and controlled
 * (`value`/`onValueChange`/`defaultValue`); Headless UI's own `TabGroup` is
 * index-based (`selectedIndex`/`onChange(index)`). Four call sites
 * (`payload-inspector.tsx`, `users-screen.tsx`, and the gallery) already
 * depend on the value-based shape, and a value-based API is more resistant
 * to bugs when tab order changes — so this adapter translates once, here,
 * rather than rewriting every call site to track an index.
 *
 * Every consumer's own JSX is untouched: only the import line changes, via
 * aliasing (`import { ValueTabs as Tabs, ... } from "@vsms/ui"`) — the one
 * genuinely necessary API break this bucket owns (see each updated file's
 * own import for the one-line diff).
 */

function collectTriggerValues(node: ReactNode, out: string[] = []): string[] {
  Children.forEach(node, (child) => {
    if (!isValidElement(child)) return;
    const el = child as ReactElement<{ value?: string; children?: ReactNode }>;
    if (el.type === ValueTabsTrigger) {
      if (typeof el.props.value === "string") out.push(el.props.value);
      return;
    }
    if (el.props?.children != null) collectTriggerValues(el.props.children, out);
  });
  return out;
}

/** Separates the one `ValueTabsList` child from everything else, so
 * `ValueTabs` can group "everything else" (the `ValueTabsContent` panels,
 * declared as direct siblings of `ValueTabsList` — the exact Radix `Tabs`
 * shape) under one implicit `TabPanels`, which Headless UI's `TabGroup`
 * requires but the old Radix-shaped call sites never had to write. */
function splitOutList(children: ReactNode): { list: ReactNode; rest: ReactNode[] } {
  let list: ReactNode = null;
  const rest: ReactNode[] = [];
  Children.forEach(children, (child) => {
    if (isValidElement(child) && child.type === ValueTabsList) {
      list = child;
    } else {
      rest.push(child);
    }
  });
  return { list, rest };
}

export interface ValueTabsProps {
  value?: string;
  defaultValue?: string;
  onValueChange?: (value: string) => void;
  className?: string;
  children: ReactNode;
}

export function ValueTabs({
  value,
  defaultValue,
  onValueChange,
  className,
  children,
}: ValueTabsProps) {
  const values = useMemo(() => collectTriggerValues(children), [children]);
  const { list, rest } = useMemo(() => splitOutList(children), [children]);
  const [internalValue, setInternalValue] = useState(defaultValue ?? values[0]);
  const isControlled = value !== undefined;
  const currentValue = isControlled ? value : internalValue;
  const selectedIndex = Math.max(0, values.indexOf(currentValue ?? values[0] ?? ""));

  function handleChange(index: number) {
    const next = values[index];
    if (next === undefined) return;
    if (!isControlled) setInternalValue(next);
    onValueChange?.(next);
  }

  return (
    <TabGroup className={className} selectedIndex={selectedIndex} onChange={handleChange}>
      {list}
      <TabPanels as={Fragment}>{rest}</TabPanels>
    </TabGroup>
  );
}

export function ValueTabsList({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <TabList className={cn("flex items-center gap-4 border-edge border-b", className)}>
      {children}
    </TabList>
  );
}

// Underline variant only (design doc §5.2: "no pill/segmented variant — pill
// tabs are consumer furniture"). 2px bottom rule in --foreground on the
// active tab.
export function ValueTabsTrigger({
  value,
  className,
  children,
}: {
  /** Consumed by `collectTriggerValues` above via prop introspection on the
   * element tree, not read inside this component itself. */
  value: string;
  className?: string;
  children: ReactNode;
}) {
  void value;
  return (
    <Tab
      type="button"
      className={cn(
        "-mb-px border-b-2 border-transparent px-1 py-2 font-medium text-body text-muted-foreground outline-none",
        "data-selected:border-foreground data-selected:text-foreground",
        className,
      )}
    >
      {children}
    </Tab>
  );
}

export function ValueTabsContent({
  className,
  children,
}: {
  /** Kept for API parity with the old value-based `TabsContent` — the real
   * Tab↔Panel association is positional (Headless UI's own `TabGroup`
   * matches `Tab`/`TabPanel` by declaration order), not looked up by value. */
  value: string;
  className?: string;
  children: ReactNode;
}) {
  return <TabPanel className={cn("pt-4", className)}>{children}</TabPanel>;
}
