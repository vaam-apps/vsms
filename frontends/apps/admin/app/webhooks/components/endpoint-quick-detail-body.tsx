import { type EndpointListItem, EVENT_TYPES } from "../webhook-domain";
import { SecretField } from "./secret-field";

// Dumb (R6): the `QuickDetailDrawer`'s summary `dl` for one endpoint.
export function EndpointQuickDetailBody({ endpoint }: { endpoint: EndpointListItem }) {
  const circuitOpen =
    endpoint.circuitOpenUntil != null && new Date(endpoint.circuitOpenUntil) > new Date();
  return (
    <dl className="flex flex-col gap-3 text-body">
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Active</dt>
        <dd>{endpoint.active ? "yes" : "no"}</dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Events</dt>
        <dd className="font-mono text-caption">
          {endpoint.eventTypes.length} of {EVENT_TYPES.length}
        </dd>
      </div>
      <div className="flex items-center justify-between gap-3">
        <dt className="text-muted-foreground">Circuit</dt>
        <dd>
          {circuitOpen ? (
            <span className="text-state-danger-fg">
              open ({endpoint.consecutiveFailures} failures)
            </span>
          ) : (
            <span className="text-muted-foreground">closed</span>
          )}
        </dd>
      </div>
      <SecretField label="Current secret" value={endpoint.secret} />
    </dl>
  );
}
