"use client";

// Route-local (R6). Covers the primitives added when this package was
// generalised for release — see `date-picker-gallery.tsx` for why a new
// export gets a gallery entry rather than shipping unmounted.

import {
  Checkbox,
  CheckboxField,
  ConfirmDialog,
  Pagination,
  Progress,
  Spinner,
  Switch,
  SwitchField,
  toast,
} from "@vaam-apps/ui";
import { useState } from "react";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

export function ControlsGallery() {
  const [checked, setChecked] = useState(true);
  const [live, setLive] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [offset, setOffset] = useState(0);
  const pageSize = 25;
  const total = 137;

  return (
    <Section
      title="Checkbox, switch, progress, pagination, confirm"
      description="A checkbox is for a value a Save button will commit; a switch promises the change already happened. Pairing a switch with a Save button tells the operator two contradictory things."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="Checkbox — on, off, indeterminate, disabled">
          <div className="flex flex-wrap items-center gap-6">
            <Checkbox checked={checked} onCheckedChange={setChecked} aria-label="Demo checkbox" />
            <Checkbox checked={false} onCheckedChange={() => undefined} aria-label="Unchecked" />
            <Checkbox
              checked={false}
              indeterminate
              onCheckedChange={() => undefined}
              aria-label="Indeterminate"
            />
            <Checkbox checked disabled onCheckedChange={() => undefined} aria-label="Disabled" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="CheckboxField — the label is itself a click target">
          <CheckboxField
            checked={checked}
            onCheckedChange={setChecked}
            label="Mask the recipient in webhook payloads"
            description="The masked form is baked in when the attempt row is written, not at delivery."
          />
        </GallerySwatch>
        <GallerySwatch label="Switch and SwitchField — takes effect immediately, no Save">
          <div className="flex max-w-md flex-col gap-4">
            <Switch checked={live} onCheckedChange={setLive} aria-label="Live updates" />
            <SwitchField
              checked={live}
              onCheckedChange={setLive}
              label="Live updates"
              description="Poll for new rows while this screen is open."
            />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Spinner — for a wait whose shape is unknown. Prefer Skeleton when it is known.">
          <div className="flex flex-wrap items-center gap-6">
            <Spinner size="xs" />
            <Spinner size="sm" />
            <Spinner size="md" />
            <Spinner size="lg" label="Loading" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Progress — a bounded, countable quantity, never an unbounded wait">
          <div className="flex max-w-md flex-col gap-3">
            <Progress value={2} max={5} label="Delivery attempts" showValue />
            <Progress value={4} max={5} tone="uncertain" label="Delivery attempts" showValue />
            <Progress value={5} max={5} tone="danger" label="Delivery attempts" showValue />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Pagination — offset mode, which needs a real total. A cursor API gets the honest `N shown` instead.">
          <div className="flex max-w-xl flex-col gap-3">
            <Pagination
              position={{ kind: "offset", offset, pageSize, total }}
              onPrevious={offset === 0 ? undefined : () => setOffset((o) => o - pageSize)}
              onNext={() => setOffset((o) => o + pageSize)}
            />
            <Pagination
              position={{ kind: "cursor", count: 25 }}
              onPrevious={undefined}
              onNext={() => undefined}
            />
          </div>
        </GallerySwatch>
        <GallerySwatch label="ConfirmDialog — for the ones worth interrupting someone over. InlineConfirm is the right default for a row action.">
          <button
            type="button"
            className="btn btn-error btn-sm"
            onClick={() => setConfirmOpen(true)}
          >
            Delete endpoint
          </button>
          <ConfirmDialog
            open={confirmOpen}
            onOpenChange={setConfirmOpen}
            tone="destructive"
            title="Delete this endpoint?"
            description="Queued attempts for it are abandoned. This cannot be undone."
            confirmLabel="Delete endpoint"
            onConfirm={() => {
              setConfirmOpen(false);
              toast({ title: "Pretend endpoint deleted", variant: "default" });
            }}
          />
        </GallerySwatch>
      </div>
    </Section>
  );
}
