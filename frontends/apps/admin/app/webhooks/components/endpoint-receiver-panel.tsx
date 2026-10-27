import { DetailList, DetailRow } from "@vaam-apps/ui";
import type { EndpointListItem } from "../webhook-domain";
import { receiverRoute } from "../webhook-domain";

// Dumb (R6): what the receiver of this endpoint has to be configured with to
// open a delivery (VS-8). A delivery is a sealed request signed with the
// gateway's own key, so nothing here is a secret: the audience and the route
// are names the receiver's envelope layer is built with, and the pinned keys
// are the ones `sms-gateway server-keys` prints.
export function EndpointReceiverPanel({ endpoint }: { endpoint: EndpointListItem }) {
  const route = receiverRoute(endpoint.url);
  return (
    <div className="flex flex-col gap-3 rounded-sm border border-edge bg-surface-2 p-3">
      <DetailList>
        <DetailRow label="Audience">
          <span className="font-mono text-caption">{endpoint.audience}</span>
        </DetailRow>
        <DetailRow label="Route">
          {route !== null ? (
            <span className="font-mono text-caption">POST {route}</span>
          ) : (
            <span className="text-state-danger-fg">
              This URL cannot be sealed: it needs a literal path with no query, credentials or
              parameters.
            </span>
          )}
        </DetailRow>
      </DetailList>
      <p className="text-caption text-subtle-foreground">
        Deliveries are signed with the gateway's own key, not a secret shared with this endpoint.
        Configure the receiver with this audience and route, and pin the gateway's public keys (
        <span className="font-mono">sms-gateway server-keys</span>); the Rust and Node SDKs open a
        delivery with a <span className="font-mono">WebhookOpener</span>.
      </p>
    </div>
  );
}
