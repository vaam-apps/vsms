// Dumb — route-local to messages (R6): the page title block. Knows only
// the poll cadence it's handed, not where it came from.

export interface MessagesHeaderProps {
  pollMs: number;
}

export function MessagesHeader({ pollMs }: MessagesHeaderProps) {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <h1 className="font-medium text-foreground text-title">Messages</h1>
      <p className="max-w-xl text-body text-muted-foreground">
        Live status of every message this app has sent — polled every ~{Math.round(pollMs / 1000)}s,
        not pushed. New rows while you're scrolled down buffer behind a pill rather than jumping the
        list.
      </p>
    </header>
  );
}
