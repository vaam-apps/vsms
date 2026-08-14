import { DetailList, DetailRow, IdDisplay, TimestampDisplay } from "@vsms/ui";
import type { RouteListItem } from "../route-domain";
import { predicateSummary } from "../route-domain";

// Dumb, route-local (R6): the `QuickDetailDrawer`'s summary `dl`.
export function RouteQuickDetailBody({ route }: { route: RouteListItem }) {
  return (
    <DetailList>
      <DetailRow label="Status">{route.enabled ? "enabled" : "disabled"}</DetailRow>
      <DetailRow label="Priority">
        <span className="font-mono">{route.priority}</span>
      </DetailRow>
      <DetailRow label="Weight">
        <span className="font-mono">{route.weight}</span>
      </DetailRow>
      <DetailRow label="Predicates" variant="stacked">
        {predicateSummary(route)}
      </DetailRow>
      <DetailRow label="Provider">
        <IdDisplay value={route.providerId} variant="full" />
      </DetailRow>
      <DetailRow label="Updated">
        <TimestampDisplay value={route.updatedAt} />
      </DetailRow>
    </DetailList>
  );
}
