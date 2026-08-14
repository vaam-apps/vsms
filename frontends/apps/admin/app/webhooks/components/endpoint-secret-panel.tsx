import { Button, TimestampDisplay } from "@vsms/ui";
import type { EndpointListItem } from "../webhook-domain";
import { SecretField } from "./secret-field";

// Dumb (R6): the current/previous secret display plus "Rotate secret" —
// the panel that sits above the endpoint's own edit form in more-detail.
export function EndpointSecretPanel({
  endpoint,
  justCreatedSecret,
  justRotatedSecret,
  onRotate,
}: {
  endpoint: EndpointListItem;
  justCreatedSecret: string | null;
  justRotatedSecret: string | null;
  onRotate: () => void;
}) {
  return (
    <>
      {justCreatedSecret != null && (
        <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
          This endpoint's secret is shown below — copy it into your receiver now. It stays visible
          via "Reveal" any time afterward (see the screen's own note on why), but this is the
          newest, safest moment to grab it.
        </div>
      )}
      {justRotatedSecret != null && (
        <div className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-3 py-2 text-caption text-state-uncertain-fg">
          Rotated. The new secret is below — copy it into your receiver. Your{" "}
          <span className="font-mono">previous secret</span> keeps verifying until the <em>next</em>{" "}
          rotation, so there is no rush, but don't wait indefinitely.
        </div>
      )}

      <div className="flex flex-col gap-3 rounded-sm border border-edge bg-surface-2 p-3">
        <SecretField label="Current secret" value={endpoint.secret} />
        {endpoint.prevSecret != null && (
          <>
            <SecretField label="Previous secret (still verifies)" value={endpoint.prevSecret} />
            <p className="text-caption text-subtle-foreground">
              Rotated{" "}
              {endpoint.secretRotatedAt != null && (
                <TimestampDisplay value={endpoint.secretRotatedAt} />
              )}{" "}
              — this value keeps accepting signatures until you rotate again. There is no fixed
              expiry.
            </p>
          </>
        )}
        <div>
          <Button type="button" variant="secondary" size="sm" onClick={onRotate}>
            Rotate secret
          </Button>
        </div>
      </div>
    </>
  );
}
