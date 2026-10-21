// Dumb — route-local to messages (R6). The sticky "N new messages" pill
// that inserts the buffered rows on click — design doc §6.5 rule 1: the
// list never auto-scrolls on its own, only on this click.

import { ArrowUp } from "lucide-react";

export interface PendingMessagesPillProps {
  count: number;
  onClick: () => void;
}

export function PendingMessagesPill({ count, onClick }: PendingMessagesPillProps) {
  if (count === 0) return null;

  return (
    <div className="-translate-x-1/2 sticky top-20 left-1/2 z-10 flex h-0 w-fit justify-center overflow-visible">
      <button
        type="button"
        onClick={onClick}
        className="mt-2 inline-flex h-10 items-center gap-2 rounded-full bg-primary px-4 font-medium text-label-large text-primary-content shadow-[var(--shadow-popover)] duration-[var(--dur-enter)] ease-out"
      >
        <ArrowUp size={16} aria-hidden="true" />
        {count} new message{count === 1 ? "" : "s"}
      </button>
    </div>
  );
}
