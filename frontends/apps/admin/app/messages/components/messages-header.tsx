// Dumb — route-local to messages (R6): the page title block. Knows only
// the poll cadence it's handed, not where it came from.
//
// The title/description pair itself is `@vsms/ui`'s `ScreenHeader` — used
// directly rather than re-hand-rolled, matching the other eight screens
// that already reach for it (`jobs-screen.tsx` and siblings). Only the
// `border-edge border-b pb-6` divider beneath it is route-local: this
// screen (and the simulator's) still wants that divider, but `ScreenHeader`
// takes no `className`, so it stays a thin wrapper rather than a reason to
// go back to hand-rolling the `<h1>`/`<p>` pair `ScreenHeader` already owns.

import { ScreenHeader } from "@vsms/ui";

export interface MessagesHeaderProps {
  pollMs: number;
}

export function MessagesHeader({ pollMs }: MessagesHeaderProps) {
  return (
    <div className="border-edge border-b pb-6">
      <ScreenHeader
        title="Messages"
        description={
          <>
            Live status of every message sent across every app in this deployment — polled every ~
            {Math.round(pollMs / 1000)}s, not pushed. New rows while you're scrolled down buffer
            behind a pill rather than jumping the list.
          </>
        }
      />
    </div>
  );
}
