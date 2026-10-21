// Dumb component (R6): the console's top app bar — M3's small top app bar,
// with the brand mark, where you are (section and page), and the theme
// toggle. It carries no navigation of its own: `SideNav` is the navigation
// at every width, and the account block lives in its `accountSlot`.
//
// The bar is sticky with a translucent fill so a long table scrolls under
// it rather than past an opaque slab. `backdrop-filter` makes it the
// containing block of any `fixed` descendant, which is harmless here: it
// has none, and the nav rails are portalled to `document.body`.

import { Button } from "@vaam-apps/ui";
import { Moon, Sun } from "lucide-react";
import Link from "next/link";

export interface TopAppBarProps {
  /** The current page's group in the navigation, e.g. "Delivery". */
  section?: string | undefined;
  /** The current page's own label, e.g. "Providers". */
  page?: string | undefined;
  /** What is painted now, so the toggle can offer the other one. */
  resolvedTheme: "light" | "dark";
  onToggleTheme: () => void;
}

export function TopAppBar({ section, page, resolvedTheme, onToggleTheme }: TopAppBarProps) {
  const next = resolvedTheme === "dark" ? "light" : "dark";
  return (
    <header className="sticky top-0 z-20 flex h-16 shrink-0 items-center gap-3 bg-base-100/85 px-4 backdrop-blur-md sm:pr-6 sm:pl-24 xl:px-10">
      <Link
        href="/"
        className="flex shrink-0 items-center gap-2.5 rounded-full pr-2 focus-visible:outline-2 focus-visible:outline-primary"
        aria-label="vsms console home"
      >
        <span
          aria-hidden="true"
          className="flex size-9 items-center justify-center rounded-[12px] bg-primary-container font-display font-semibold text-on-primary-container text-title-medium"
        >
          v
        </span>
        <span className="font-display font-semibold text-title-medium tracking-tight">vsms</span>
      </Link>
      {page !== undefined && (
        <nav aria-label="Breadcrumb" className="hidden min-w-0 items-center gap-2 md:flex">
          <span aria-hidden="true" className="h-5 w-px bg-edge" />
          {section !== undefined && (
            <span className="truncate text-label-large text-muted-foreground">{section}</span>
          )}
          {section !== undefined && (
            <span aria-hidden="true" className="text-muted-foreground">
              /
            </span>
          )}
          <span aria-current="page" className="truncate font-medium text-label-large">
            {page}
          </span>
        </nav>
      )}
      <div className="ml-auto flex items-center gap-1">
        <Button
          variant="ghost"
          size="icon"
          type="button"
          onClick={onToggleTheme}
          aria-label={`Switch to ${next} theme`}
          title={`Switch to ${next} theme`}
        >
          {resolvedTheme === "dark" ? (
            <Sun size={20} aria-hidden="true" />
          ) : (
            <Moon size={20} aria-hidden="true" />
          )}
        </Button>
      </div>
    </header>
  );
}
