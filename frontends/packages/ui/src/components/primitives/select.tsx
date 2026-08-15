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
    // `as="div"` + `relative`: with `SelectContent` no longer portaled
    // (see its own comment), the options position themselves against this
    // element. Headless UI's `Listbox` renders a fragment by default, which
    // would leave `absolute` resolving against whatever ancestor happened
    // to be positioned — usually the drawer, putting the dropdown in the
    // wrong place entirely.
    <Listbox
      as="div"
      className="relative"
      value={currentValue ?? ""}
      onChange={handleChange}
      {...omitUndefined({ disabled })}
    >
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
      // `portal={false}` is a correctness fix, not a preference.
      //
      // Headless UI's `anchor` prop portals the options into
      // `#headlessui-portal-root`, a top-level sibling of `<body>`. Inside a
      // `vaul` drawer that is fatal: vaul's `Content` mounts a Radix
      // `FocusScope` with `trapped: true`, a document-level `focusin`
      // listener that force-refocuses back into the drawer the instant
      // focus lands outside it. The portaled listbox is outside, so its
      // enter transition stalls mid-flight and it never becomes usable.
      //
      // Measured on the live console rather than inferred — opening the
      // registration status select inside its stacked drawer gave:
      //
      //   parentChain     [..., DIV#headlessui-portal-root, BODY]
      //   insideAnyDrawer false
      //   opacity         "0"
      //   pointerEvents   "none"
      //   rect            [122, 10]   (collapsed, not four options tall)
      //
      // — byte-for-byte the signature #274 recorded for a nested Dialog.
      // #282 fixed that class for `Dialog` and never covered `Select`, so
      // every select inside a drawer has been silently unusable since.
      //
      // Rendering inline keeps the options inside the drawer's own subtree,
      // so the focus trap contains them instead of fighting them. The cost
      // is losing `anchor`'s collision detection; `top-full` + `w-full`
      // below reproduces the same "directly under the trigger, matching its
      // width" placement, which is what every call site here wants anyway.
      portal={false}
      // No `transition`, and no `data-closed:*` classes. Headless UI's
      // transition machinery holds `data-closed` until it observes the
      // enter transition finish; inside a drawer it never does. Measured
      // after the portal fix, 1.4s after opening: `data-closed` and
      // `data-enter` both still present, `opacity: 0`, on an element that
      // was otherwise correct (inside the drawer, `pointer-events: auto`,
      // full 146px height, all four options present).
      //
      // That is the same failure mode twice now — #274's nested Dialog was
      // also a stalled enter transition, not a mispositioned element. A
      // 100ms fade on a select dropdown is not worth a second component
      // that silently does not open, so it is gone rather than debugged.
      className={cn(
        "absolute top-full left-0 z-50 mt-1 max-h-80 w-full min-w-[8rem] overflow-y-auto rounded-md border border-edge bg-surface-2 p-1 shadow-[var(--shadow-popover)] focus:outline-none",
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
