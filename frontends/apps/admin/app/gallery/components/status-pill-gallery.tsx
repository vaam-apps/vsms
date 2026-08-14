"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { MESSAGE_STATES, StatusPill, toast } from "@vsms/ui";
import { useState } from "react";
import { Section } from "./section";

const QUIET_STATES = MESSAGE_STATES.filter((s) =>
  ["accepted", "queued", "routed", "submitted", "delivered", "cancelled"].includes(s),
);
const LOUD_STATES = MESSAGE_STATES.filter((s) =>
  ["uncertain", "undelivered", "failed", "expired", "rejected"].includes(s),
);

export function StatusPillGallery() {
  const [grayscale, setGrayscale] = useState(false);

  return (
    <Section
      title="Status system — eleven states"
      description="Every state messages_state_enum_check can produce, rendered in its natural (§4.5) attention treatment. delivered carries the owner's green-pill override; the rest of the ladder is unchanged."
    >
      <label className="flex w-fit items-center gap-2 text-caption text-muted-foreground">
        <input
          type="checkbox"
          checked={grayscale}
          onChange={(e) => setGrayscale(e.target.checked)}
          className="checkbox checkbox-sm"
        />
        Accessibility check: render at grayscale(1) — all eleven must stay distinguishable (§4.6)
      </label>
      <div className={grayscale ? "grayscale" : undefined}>
        <div className="flex flex-col gap-4">
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Quiet — on track / uneventful terminal
            </p>
            <div className="flex flex-wrap gap-3">
              {QUIET_STATES.map((s) => (
                <StatusPill key={s} state={s} showLiteral />
              ))}
            </div>
          </div>
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Loud — needs a human
            </p>
            <div className="flex flex-wrap gap-3">
              {LOUD_STATES.map((s) => (
                <StatusPill key={s} state={s} showLiteral />
              ))}
            </div>
          </div>
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Other `StatusPill` states: pending (optimistic, unconfirmed) and interactive
              (clickable)
            </p>
            <div className="flex flex-wrap items-center gap-3">
              <StatusPill state="queued" pending showLiteral detail="optimistic" />
              <StatusPill
                state="failed"
                interactive
                showLiteral
                detail="click me"
                onClick={() => toast({ title: "StatusPill clicked", variant: "default" })}
              />
            </div>
          </div>
        </div>
      </div>
    </Section>
  );
}
