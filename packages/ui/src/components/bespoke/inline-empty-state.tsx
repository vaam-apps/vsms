import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

export interface InlineEmptyStateProps {
  message: ReactNode;
  action?: { label: string; onClick: () => void };
  /** `'standalone'` is the one exception (§5.3): a screen with nothing else
   * to do may centre a single line plus one primary action. Still no
   * illustration, still no card. */
  variant?: "inline" | "standalone";
  className?: string;
}

/**
 * Enforces the binding rule: empty states are inline status lines, not
 * centred placards. Use inside a table body (spanning all columns) or in
 * the panel where the missing list would be.
 */
export function InlineEmptyState({
  message,
  action,
  variant = "inline",
  className,
}: InlineEmptyStateProps) {
  return (
    <div
      className={cn(
        "flex items-center gap-3 text-body text-muted-foreground",
        variant === "standalone" ? "justify-center py-16 text-center" : "py-3 text-left",
        className,
      )}
    >
      <span>{message}</span>
      {action != null && (
        <button
          type="button"
          onClick={action.onClick}
          className="text-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
        >
          {action.label}
        </button>
      )}
    </div>
  );
}
