// Dumb view: the screen title, "New app" button, and the reads-act-as-you
// permission note.

import { Button } from "@vsms/ui";

export function AppsHeader({ onCreateClick }: { onCreateClick: () => void }) {
  return (
    <>
      <div className="flex flex-col items-start justify-between gap-4 border-edge border-b pb-6 sm:flex-row sm:items-center">
        <div>
          <h1 className="font-medium text-foreground text-title">Apps</h1>
          <p className="mt-1 max-w-xl text-body text-muted-foreground">
            Every integrated product, its quota, and its service-account clients.
          </p>
        </div>
        <Button type="button" onClick={onCreateClick} className="shrink-0">
          New app
        </Button>
      </div>

      <div className="rounded-sm border border-edge bg-surface-2 px-3 py-2 text-caption text-muted-foreground">
        Reads and writes act as you — saving requires your own role to carry{" "}
        <span className="font-mono text-foreground">app:write</span> (owner and admin by default),
        and provisioning/retiring a client needs{" "}
        <span className="font-mono text-foreground">user:manage</span>-adjacent trust: this
        console&apos;s own permission table gates it at{" "}
        <span className="font-mono text-foreground">owner</span>/
        <span className="font-mono text-foreground">admin</span> only.
      </div>
    </>
  );
}
