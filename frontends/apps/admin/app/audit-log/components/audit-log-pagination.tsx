// Dumb view: the offset pager beneath the audit log table.

import { Button } from "@vsms/ui";

export function AuditLogPagination({
  shownCount,
  offset,
  hasMore,
  onPrevious,
  onNext,
}: {
  shownCount: number;
  offset: number;
  hasMore: boolean;
  onPrevious: () => void;
  onNext: () => void;
}) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-caption text-subtle-foreground">
        Showing {shownCount} entries starting at offset {offset}
      </span>
      <div className="flex gap-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          disabled={offset === 0}
          onClick={onPrevious}
        >
          Previous
        </Button>
        <Button type="button" variant="secondary" size="sm" disabled={!hasMore} onClick={onNext}>
          Next
        </Button>
      </div>
    </div>
  );
}
