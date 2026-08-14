import { Button } from "../primitives/button";
import { InlineBanner } from "./inline-banner";

/**
 * "Someone else changed this row since it loaded" — the banner for a
 * `412 Precondition Failed` save.
 *
 * Shared rather than route-local because the thing it reports is not
 * route-local: #59 gave `@version` to ten operator-editable models and
 * threaded ETag/`If-Match` through the generated REST layer, so *every*
 * edit screen in the console can lose this race. `apps/` and `users/` each
 * hand-rolled it; the rest will need it as their edit screens land.
 *
 * The two hand-rolled copies differed in exactly one way — one hardcoded
 * its sentence, the other took a `message` prop. The prop version is
 * strictly more general, so it wins, with the hardcoded sentence as the
 * default so the common case stays a one-line call.
 *
 * `warning`, not `danger`: nothing was lost and nothing is broken. The
 * write was *refused* precisely so the other operator's edit survives —
 * which is the mechanism working, not failing. Reloading resolves it.
 * `packages/gateway/src/errors.ts`'s `isStaleWriteError` exists to
 * distinguish this from a genuine conflict for the same reason.
 */
export function StaleWriteBanner({
  message = "Someone else changed this row since it loaded. Reload to see their edit.",
  onReload,
}: {
  message?: string | undefined;
  onReload: () => void;
}) {
  return (
    <InlineBanner variant="warning" className="flex items-center justify-between gap-3">
      <span>{message}</span>
      {/* `type="button"` is load-bearing, not boilerplate: both call sites
          render this *inside* a `<form>`, where a button's default type is
          `submit`. Dropping it would make "Reload" submit the very edit the
          banner is warning has gone stale. */}
      <Button type="button" variant="secondary" size="sm" onClick={onReload}>
        Reload
      </Button>
    </InlineBanner>
  );
}
