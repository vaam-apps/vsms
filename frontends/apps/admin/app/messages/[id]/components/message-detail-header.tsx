// Dumb — route-local to the message detail screen (R6). Title, the raw id
// (before the record itself has even loaded), and the back link.

export interface MessageDetailHeaderProps {
  messageId: string;
}

export function MessageDetailHeader({ messageId }: MessageDetailHeaderProps) {
  return (
    <header className="flex flex-col gap-3 border-edge border-b pb-6 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
      <div className="min-w-0">
        <h1 className="font-medium text-foreground text-title">Message detail</h1>
        <p className="mt-1 max-w-xl break-all text-body text-muted-foreground">{messageId}</p>
      </div>
      <div className="flex shrink-0 items-center gap-3">
        {/* Matches the rest of this console's internal navigation — a
         * plain `<a>`, not `next/link`'s `Link` (see `messages-table.tsx`'s
         * own note on why). */}
        <a
          href="/messages"
          className="text-caption text-muted-foreground underline decoration-edge-strong underline-offset-2 hover:decoration-foreground"
        >
          ← Back to messages
        </a>
      </div>
    </header>
  );
}
