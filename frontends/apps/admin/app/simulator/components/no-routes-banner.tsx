// Dumb component (R6): the "zero Route rows exist at all" banner — distinct
// from "routes exist, none matched" (`EvaluationsTable`'s own empty state).
// Static content, no props. Moved verbatim out of `simulator-screen.tsx`.

export function NoRoutesBanner() {
  return (
    <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
      No <span className="font-mono">Route</span> rows exist in this system at all — every message
      would be rejected, loudly (§62's own "dispatch refuses, not silently falls back"). This is
      distinct from "routes exist, none matched" below — there is nothing at all to evaluate here.
    </div>
  );
}
