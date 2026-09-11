"use client";

import { ChevronLeft, ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Button } from "./button";

export interface PaginationProps {
  onPrevious: (() => void) | undefined;
  onNext: (() => void) | undefined;
  /**
   * What the reader is looking at. Two honest shapes, and the choice is
   * not cosmetic:
   *
   * - `{ kind: "offset", offset, pageSize, total }` — only when the API
   *   genuinely returns a total. Renders `1–25 of 340`.
   * - `{ kind: "cursor", count }` — a keyset-paged API, which most
   *   event-shaped endpoints are, because counting a large table on every
   *   request is the expensive part. Renders `25 shown` and no page
   *   numbers, because there is no honest page number to render.
   *
   * Inventing a page count from a cursor API is the failure this type
   * exists to prevent: it produces a "Page 3 of ?" that is wrong the
   * moment a row is inserted.
   */
  position: PaginationPosition;
  /** Rendered between the counts and the buttons — a page-size select, a
   * "Newest first" note. */
  children?: ReactNode;
  className?: string | undefined;
}

export type PaginationPosition =
  | { kind: "offset"; offset: number; pageSize: number; total: number }
  | { kind: "cursor"; count: number };

function positionLabel(position: PaginationPosition): string {
  if (position.kind === "cursor") {
    return position.count === 0 ? "No results" : `${position.count} shown`;
  }
  const { offset, pageSize, total } = position;
  if (total === 0) return "No results";
  const first = offset + 1;
  const last = Math.min(offset + pageSize, total);
  return `${first}–${last} of ${total}`;
}

/**
 * Previous/next paging with an honest position readout.
 *
 * Both callbacks are required but nullable: `undefined` disables that
 * direction. Making them required-but-nullable rather than optional
 * forces a caller to decide what the boundary is, instead of getting a
 * silently dead button by omission.
 */
export function Pagination({ onPrevious, onNext, position, children, className }: PaginationProps) {
  const atEnd =
    position.kind === "cursor"
      ? onNext === undefined
      : position.offset + position.pageSize >= position.total;

  return (
    <nav
      aria-label="Pagination"
      className={cn("flex items-center justify-between gap-4 text-caption", className)}
    >
      <p className="font-mono text-subtle-foreground tabular-nums">{positionLabel(position)}</p>
      <div className="flex items-center gap-2">
        {children}
        <Button
          variant="secondary"
          size="sm"
          onClick={onPrevious}
          disabled={onPrevious === undefined}
          aria-label="Previous page"
        >
          <ChevronLeft size={14} strokeWidth={1.5} />
          Previous
        </Button>
        <Button
          variant="secondary"
          size="sm"
          onClick={onNext}
          disabled={onNext === undefined || atEnd}
          aria-label="Next page"
        >
          Next
          <ChevronRight size={14} strokeWidth={1.5} />
        </Button>
      </div>
    </nav>
  );
}
