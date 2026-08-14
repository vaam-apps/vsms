"use client";

import {
  DialogBackdrop,
  DialogPanel,
  Dialog as HeadlessDialog,
  DialogDescription as HeadlessDialogDescription,
  DialogTitle as HeadlessDialogTitle,
} from "@headlessui/react";
import { X } from "lucide-react";
import {
  type ComponentPropsWithoutRef,
  createContext,
  type ElementType,
  type MouseEvent,
  type ReactNode,
  useContext,
  useState,
} from "react";
import { cn } from "../../lib/cn";

interface DialogContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
}
const DialogContext = createContext<DialogContextValue | null>(null);

function useDialogContext(component: string): DialogContextValue {
  const ctx = useContext(DialogContext);
  if (ctx === null) {
    throw new Error(`<${component} /> must be rendered inside <Dialog>.`);
  }
  return ctx;
}

export interface DialogProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  children: ReactNode;
}

/**
 * D3: Radix `Dialog` → Headless UI `Dialog`/`DialogPanel`/`DialogTitle`/`DialogBackdrop`.
 *
 * `Dialog` itself is **not** a Headless UI component — it's a plain context
 * provider. Headless UI's own `Dialog` only wraps the modal
 * backdrop/panel (`DialogContent` below) and is controlled-only
 * (`open`/`onClose`); it has no equivalent to Radix's `Dialog.Trigger`,
 * which rendered in place (outside any portal) and could itself drive an
 * *uncontrolled* open state. Two real consumers need both shapes:
 * `providers-screen.tsx`'s edit dialog is fully controlled
 * (`open={selectedId !== null}`, no `DialogTrigger` at all), while the
 * gallery's demo dialog is uncontrolled, opened by its own in-place
 * `DialogTrigger`. This root supplies the open/close plumbing for both,
 * the same controlled/uncontrolled duality `Select` (D17) uses.
 */
export function Dialog({ open, defaultOpen = false, onOpenChange, children }: DialogProps) {
  const [internalOpen, setInternalOpen] = useState(defaultOpen);
  const isControlled = open !== undefined;
  const currentOpen = isControlled ? open : internalOpen;

  function setOpen(next: boolean) {
    if (!isControlled) setInternalOpen(next);
    onOpenChange?.(next);
  }

  return (
    <DialogContext.Provider value={{ open: currentOpen, setOpen }}>
      {children}
    </DialogContext.Provider>
  );
}

type TriggerProps<T extends ElementType> = { as?: T } & Omit<ComponentPropsWithoutRef<T>, "as">;

/**
 * Renders in place, not portaled — `as` selects the rendered element/component,
 * matching Headless UI's own polymorphism convention rather than Radix's
 * `asChild` (D4 already retired that pattern for `Button`; this is the
 * identical call applied to a trigger). Replaces
 * `<DialogTrigger asChild><Button variant="secondary">…</Button></DialogTrigger>`
 * with `<DialogTrigger as={Button} variant="secondary">…</DialogTrigger>`
 * (the gallery's own only consumer, updated in this same change).
 */
export function DialogTrigger<T extends ElementType = "button">({
  as,
  onClick,
  ...props
}: TriggerProps<T>) {
  const { setOpen } = useDialogContext("DialogTrigger");
  const Component = (as ?? "button") as ElementType;
  return (
    <Component
      type={as === undefined ? "button" : undefined}
      onClick={(event: MouseEvent) => {
        (onClick as ((e: MouseEvent) => void) | undefined)?.(event);
        setOpen(true);
      }}
      {...props}
    />
  );
}

/** Unconsumed today (grepped across `admin/`) — kept for API parity. Same
 * `as`-polymorphic shape as `DialogTrigger`. */
export function DialogClose<T extends ElementType = "button">({
  as,
  onClick,
  ...props
}: TriggerProps<T>) {
  const { setOpen } = useDialogContext("DialogClose");
  const Component = (as ?? "button") as ElementType;
  return (
    <Component
      type={as === undefined ? "button" : undefined}
      onClick={(event: MouseEvent) => {
        (onClick as ((e: MouseEvent) => void) | undefined)?.(event);
        setOpen(false);
      }}
      {...props}
    />
  );
}

// Radius bumps to --radius-md here on purpose (design doc §3.5, unchanged by
// D8's radius-scale rewrite — see theme.css): a floating layer reads as
// detached from the grid beneath it.
export function DialogContent({ className, children, ...props }: ComponentPropsWithoutRef<"div">) {
  const { open, setOpen } = useDialogContext("DialogContent");
  return (
    <HeadlessDialog open={open} onClose={setOpen} className="relative z-50">
      <DialogBackdrop
        transition
        className="fixed inset-0 bg-black/50 duration-150 ease-out data-closed:opacity-0"
      />
      <div className="fixed inset-0 flex w-screen items-center justify-center p-4">
        <DialogPanel
          transition
          className={cn(
            "relative w-full max-w-[480px] rounded-md border border-edge bg-surface-2 p-6 shadow-[var(--shadow-dialog)]",
            "duration-150 ease-out data-closed:scale-95 data-closed:opacity-0",
            className,
          )}
          {...props}
        >
          {children}
          <button
            type="button"
            aria-label="Close"
            onClick={() => setOpen(false)}
            className="absolute top-4 right-4 text-subtle-foreground hover:text-foreground"
          >
            <X size={16} strokeWidth={1.5} />
          </button>
        </DialogPanel>
      </div>
    </HeadlessDialog>
  );
}

export function DialogHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("mb-4 flex flex-col gap-1", className)} {...props} />;
}

/** #56: the confirm/requeue dialog's own action row. Mirrors `DialogHeader`'s
 * shape (a thin, class-composing div, not a Headless-UI-wrapped primitive —
 * a footer has no accessibility semantics either library needs to own). */
export function DialogFooter({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("mt-6 flex items-center justify-end gap-2", className)} {...props} />;
}

export function DialogTitle({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof HeadlessDialogTitle>) {
  return (
    <HeadlessDialogTitle
      className={cn("font-medium text-foreground text-title-sm", className)}
      {...props}
    />
  );
}

export function DialogDescription({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof HeadlessDialogDescription>) {
  return (
    <HeadlessDialogDescription
      className={cn("text-muted-foreground text-prose", className)}
      {...props}
    />
  );
}
