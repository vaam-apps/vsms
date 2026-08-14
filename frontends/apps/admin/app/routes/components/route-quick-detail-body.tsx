import { IdDisplay, TimestampDisplay } from "@vsms/ui";
import type { RouteListItem } from "../route-domain";
import { predicateSummary } from "../route-domain";

// Dumb, route-local (R6): the `QuickDetailDrawer`'s summary `dl`.
export function RouteQuickDetailBody({ route }: { route: RouteListItem }) {
  return (
    <dl className="flex flex-col gap-3 text-body">
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Status</dt>
        <dd>{route.enabled ? "enabled" : "disabled"}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Priority</dt>
        <dd className="font-mono">{route.priority}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Weight</dt>
        <dd className="font-mono">{route.weight}</dd>
      </div>
      <div className="flex flex-col gap-1">
        <dt className="text-muted-foreground">Predicates</dt>
        <dd className="text-caption">{predicateSummary(route)}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Provider</dt>
        <dd>
          <IdDisplay value={route.providerId} variant="full" />
        </dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Updated</dt>
        <dd>
          <TimestampDisplay value={route.updatedAt} />
        </dd>
      </div>
    </dl>
  );
}
