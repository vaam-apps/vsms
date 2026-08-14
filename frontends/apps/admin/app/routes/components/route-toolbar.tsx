import { Button, InlineBanner } from "@vsms/ui";

// Dumb, route-local (R6): the role-scope notice, the "no routes at all"
// danger banner (§62 — every message is refused while this is true), a list
// read-error banner, and the "New route" action. Grouped into one component
// because all four are static or thin conditionals over the same handful of
// booleans the screen already has on hand from its own query — not because
// they're one visual unit.
export function RouteToolbar({
  isEmpty,
  listErrorMessage,
  onNewRoute,
}: {
  isEmpty: boolean;
  listErrorMessage?: string | undefined;
  onNewRoute: () => void;
}) {
  return (
    <>
      <InlineBanner variant="neutral">
        Create/Save/Delete act as you, not as a shared service account — they require your own role
        to be <span className="font-mono text-foreground">owner</span> or{" "}
        <span className="font-mono text-foreground">admin</span>; other roles (including operator)
        will see a real <span className="font-mono text-foreground">Forbidden</span> here.
      </InlineBanner>

      {isEmpty && (
        <InlineBanner variant="danger">
          No routes configured at all — every message this system accepts is refused, loudly (§62).
          At least one enabled route is required before anything can be dispatched.
        </InlineBanner>
      )}

      {listErrorMessage != null && (
        <InlineBanner variant="danger">Could not read routes: {listErrorMessage}</InlineBanner>
      )}

      <div>
        <Button type="button" onClick={onNewRoute}>
          New route
        </Button>
      </div>
    </>
  );
}
