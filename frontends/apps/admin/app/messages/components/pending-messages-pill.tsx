// Dumb — route-local to messages (R6). The sticky "N new messages" pill
// that inserts the buffered rows on click — design doc §6.5 rule 1: the
// list never auto-scrolls on its own, only on this click.

export interface PendingMessagesPillProps {
  count: number;
  onClick: () => void;
}

export function PendingMessagesPill({ count, onClick }: PendingMessagesPillProps) {
  if (count === 0) return null;

  return (
    <div className="-translate-x-1/2 sticky top-2 left-1/2 z-20 flex w-fit justify-center">
      <button
        type="button"
        onClick={onClick}
        className="rounded-full border border-edge bg-surface-2 px-3 py-1 text-caption text-foreground shadow-[var(--shadow-popover)] duration-[var(--dur-enter)] ease-out"
      >
        {count} new message{count === 1 ? "" : "s"}
      </button>
    </div>
  );
}
