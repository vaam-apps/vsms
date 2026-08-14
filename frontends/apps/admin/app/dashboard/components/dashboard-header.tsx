// Dumb component (R6): the Dashboard screen's own title block. Markup and
// classes moved verbatim out of `dashboard-screen.tsx`.

export interface DashboardHeaderProps {
  /** The screen's own poll cadence, so the copy below never drifts from
   * the actual `refetchInterval` the smart component passes to its query. */
  refetchIntervalMs: number;
}

export function DashboardHeader({ refetchIntervalMs }: DashboardHeaderProps) {
  return (
    <header className="flex flex-col gap-1 border-edge border-b pb-6">
      <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
        vsms admin console
      </p>
      <h1 className="font-medium text-foreground text-title">Dashboard</h1>
      <p className="max-w-xl text-body text-muted-foreground">
        Throughput, delivery, and backlog at a glance. Refreshes every{" "}
        {Math.round(refetchIntervalMs / 1000)}s.
      </p>
    </header>
  );
}
