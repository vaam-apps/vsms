"use client";

import { useSyncExternalStore } from "react";
import { cn } from "../../lib/cn";

/**
 * Toasts, no Radix — `dialog`/`dropdown-menu`/`select`/`tooltip`/`popover`
 * are the five primitives that need Radix's behaviour (design doc T6
 * brief); a toast is a transient, non-modal, non-focus-trapping
 * notification, so a small hand-rolled store is enough.
 *
 * For transient confirmations only ("copied", "saved", "replay queued") —
 * design doc §5.1: anything an operator must act on is inline, never a
 * toast, because a toast that expires while they're reading a payload is a
 * lost message.
 */

export type ToastVariant = "default" | "success" | "danger";

export interface ToastItem {
  id: string;
  title: string;
  description?: string;
  variant?: ToastVariant;
  durationMs?: number;
}

type Listener = () => void;

let toasts: ToastItem[] = [];
const listeners = new Set<Listener>();

function emit() {
  for (const listener of listeners) listener();
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): ToastItem[] {
  return toasts;
}

export function dismissToast(id: string) {
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

export function toast(item: Omit<ToastItem, "id">): string {
  const id = crypto.randomUUID();
  const durationMs = item.durationMs ?? 4000;
  toasts = [...toasts, { ...item, id }];
  emit();
  if (durationMs > 0) {
    setTimeout(() => dismissToast(id), durationMs);
  }
  return id;
}

const VARIANT_CLASSES: Record<ToastVariant, string> = {
  default: "border-edge bg-surface-2 text-foreground",
  success: "border-state-success-fg/30 bg-surface-2 text-foreground",
  danger: "border-state-danger-border bg-state-danger-bg text-state-danger-fg",
};

/** Mount once, near the app root. Renders the live toast stack. */
export function Toaster() {
  const items = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2"
    >
      {items.map((item) => (
        <div
          key={item.id}
          className={cn(
            "pointer-events-auto rounded-sm border p-3 text-body shadow-[var(--shadow-popover)]",
            VARIANT_CLASSES[item.variant ?? "default"],
          )}
        >
          <div className="flex items-start justify-between gap-2">
            <p className="font-medium">{item.title}</p>
            <button
              type="button"
              onClick={() => dismissToast(item.id)}
              aria-label="Dismiss"
              className="text-subtle-foreground hover:text-foreground"
            >
              ×
            </button>
          </div>
          {item.description != null && (
            <p className="mt-1 text-caption text-muted-foreground">{item.description}</p>
          )}
        </div>
      ))}
    </div>
  );
}
