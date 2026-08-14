// Dumb component (R6): the "Recent" section heading plus the "Record
// opt-out" trigger, moved verbatim out of `opt-outs-screen.tsx`.

import { Button } from "@vsms/ui";

export interface OptOutsToolbarProps {
  onRecordClick: () => void;
}

export function OptOutsToolbar({ onRecordClick }: OptOutsToolbarProps) {
  return (
    <div className="flex items-center justify-between">
      <h2 className="font-medium text-body text-foreground">Recent</h2>
      <Button type="button" onClick={onRecordClick}>
        Record opt-out
      </Button>
    </div>
  );
}
