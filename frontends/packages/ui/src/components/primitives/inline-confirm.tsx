"use client";

import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { InlineBanner } from "../bespoke/inline-banner";
import { Button } from "./button";

/**
 * A confirmation rendered **inline**, inside the caller's own DOM subtree —
 * never a portal, never a second `Dialog`/`FocusScope`. Exists specifically
 * to replace a centered `Dialog` nested inside an open `QuickDetailDrawer` /
 * `MoreDetailDrawer` (`vaul`), which is permanently broken: see
 * `frontends/apps/admin/app/gallery/page.tsx`'s own
 * `NestedDialogInDrawerRegression` writeup for the full root-cause
 * investigation (`vaul`'s `Content` mounts a trapped, document-level
 * `@radix-ui/react-focus-scope` `FocusScope` regardless of `dimmed`/`modal`,
 * and Headless UI's `Dialog` always portals to its own sibling
 * `#headlessui-portal-root` — so the trap yanks focus straight back out of
 * the portal the instant it tries to move in, and the confirmation never
 * becomes visible).
 *
 * Covers both destructive yes/no confirmations (`title`/`description`, no
 * `children`) and a short form embedded in a confirmation step (`children`
 * holds the field, e.g. sender-ids-screen.tsx's provider picker) — the two
 * shapes this console actually needs, one component. A second, form-only
 * component was considered and rejected: both shapes are "explain the
 * consequence, then commit or back out," and forking that into two
 * components would just be this file's own title/description/actions
 * scaffold copy-pasted around a different middle section.
 *
 * The caller owns visibility: render this instead of the drawer's normal
 * body content (and drop the drawer's own `footer` prop, since this
 * component supplies its own Cancel/Confirm row) rather than layering it on
 * top — there is no overlay, no absolute positioning, and no independent
 * open/close transition to fight the drawer's own.
 */
export interface InlineConfirmProps {
  /** The question being asked, e.g. "Delete this route?" or "Register {value}
   * with a provider". Rendered as the panel's own heading — the drawer's
   * `title` prop stays whatever record is being acted on; this is the
   * action, not the record. */
  title: ReactNode;
  description?: ReactNode;
  /** A field belonging to this confirmation, e.g. a provider `Select` — omit
   * for a plain yes/no confirm. */
  children?: ReactNode;
  confirmLabel: string;
  /** Shown on the confirm button while `pending` — defaults to `confirmLabel`
   * with an ellipsis. */
  pendingLabel?: string | undefined;
  cancelLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
  pending?: boolean;
  /** Disables the confirm button independent of `pending`, e.g. no provider
   * selected yet. */
  confirmDisabled?: boolean;
  /** Danger-hued confirm button + accent border, matching `Dialog`'s own
   * "destructive action in the danger hue, never the primary hue"
   * convention (console-redesign.md §1.7). Defaults `true` — every current
   * call site is destructive; the one exception (sender-ids-screen.tsx's
   * "register with a provider" form) passes `false`. */
  destructive?: boolean;
  error?: ReactNode;
  className?: string;
}

export function InlineConfirm({
  title,
  description,
  children,
  confirmLabel,
  pendingLabel,
  cancelLabel = "Cancel",
  onConfirm,
  onCancel,
  pending = false,
  confirmDisabled = false,
  destructive = true,
  error,
  className,
}: InlineConfirmProps) {
  return (
    <div
      className={cn(
        "flex flex-col gap-4 rounded-sm border p-4",
        destructive
          ? "border-state-danger-border bg-state-danger-bg/30"
          : "border-edge bg-surface-2",
        className,
      )}
    >
      <div className="flex flex-col gap-1">
        <h3 className="font-medium text-foreground text-title-sm">{title}</h3>
        {description != null && <p className="text-body text-muted-foreground">{description}</p>}
      </div>

      {children != null && <div className="flex flex-col gap-4">{children}</div>}

      {error != null && <InlineBanner variant="danger">{error}</InlineBanner>}

      <div className="flex items-center justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" disabled={pending} onClick={onCancel}>
          {cancelLabel}
        </Button>
        <Button
          type="button"
          variant={destructive ? "destructive" : "primary"}
          size="sm"
          disabled={pending || confirmDisabled}
          onClick={onConfirm}
        >
          {pending ? (pendingLabel ?? `${confirmLabel}…`) : confirmLabel}
        </Button>
      </div>
    </div>
  );
}
