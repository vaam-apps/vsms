import { DetailList, DetailRow } from "@vsms/ui";
import { type EndpointListItem, EVENT_TYPES } from "../webhook-domain";
import { SecretField } from "./secret-field";

// Dumb (R6): the `QuickDetailDrawer`'s summary `dl` for one endpoint.
export function EndpointQuickDetailBody({ endpoint }: { endpoint: EndpointListItem }) {
  const circuitOpen =
    endpoint.circuitOpenUntil != null && new Date(endpoint.circuitOpenUntil) > new Date();
  return (
    <DetailList>
      <DetailRow label="Active">{endpoint.active ? "yes" : "no"}</DetailRow>
      <DetailRow label="Events">
        <span className="font-mono text-caption">
          {endpoint.eventTypes.length} of {EVENT_TYPES.length}
        </span>
      </DetailRow>
      <DetailRow label="Circuit">
        {circuitOpen ? (
          <span className="text-state-danger-fg">
            open ({endpoint.consecutiveFailures} failures)
          </span>
        ) : (
          <span className="text-muted-foreground">closed</span>
        )}
      </DetailRow>
      <SecretField label="Current secret" value={endpoint.secret} />
    </DetailList>
  );
}
