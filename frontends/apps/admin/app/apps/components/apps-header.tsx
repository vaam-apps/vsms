// Dumb view: the screen title, "New app" button, and the reads-act-as-you
// permission note.
//
// `ScreenHeader` (`@vsms/ui`) has no action-button slot, so the title/
// description pair and the button share one flex row here — same layout
// `sender-ids-screen.tsx`'s own `SenderToolbar` establishes for the
// identical "title + description + button" shape, just inlined rather
// than split into a second component since this route has only one
// toolbar action.

import { Button, Code, InlineBanner, ScreenHeader } from "@vsms/ui";

export function AppsHeader({ onCreateClick }: { onCreateClick: () => void }) {
  return (
    <>
      <div className="flex flex-col items-start justify-between gap-4 sm:flex-row sm:items-center">
        <ScreenHeader
          title="Apps"
          description="Every integrated product, its quota, and its service-account clients."
        />
        <Button type="button" onClick={onCreateClick} className="shrink-0">
          New app
        </Button>
      </div>

      <InlineBanner variant="neutral">
        Reads and writes act as you — saving requires your own role to carry <Code>app:write</Code>{" "}
        (owner and admin by default), and provisioning/retiring a client needs{" "}
        <Code>user:manage</Code>-adjacent trust: this console&apos;s own permission table gates it
        at <Code>owner</Code>/<Code>admin</Code> only.
      </InlineBanner>
    </>
  );
}
