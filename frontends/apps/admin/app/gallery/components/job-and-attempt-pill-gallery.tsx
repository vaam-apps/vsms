"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { ATTEMPT_STATES, AttemptStatusPill, JOB_STATES, JobStatusPill } from "@vsms/ui";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

/**
 * `JobStatusPill`/`AttemptStatusPill` (#56/#55) — two more state machines
 * with their own transitions table, deliberately not folded into the
 * message pill above (`status-tokens.ts`'s own module doc: a job's
 * `failed` is retryable, a message's `failed` is terminal). Neither was
 * mounted anywhere in this gallery before this pass — a real coverage gap,
 * found by cross-checking `@vsms/ui`'s index against this file's own
 * imports rather than assumed complete.
 */
export function JobAndAttemptPillGallery() {
  return (
    <Section
      title="Job and attempt status pills"
      description="Six job states, five attempt states — same glyph/hue system as StatusPill, driven by JOB_STATUS_META / ATTEMPT_STATUS_META instead of MESSAGE_STATUS_META. Note failed here is retryable (unresolved/uncertain hue), not the terminal danger hue a message's own failed carries."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="Job — pending / running / succeeded / failed (retrying) / dead / cancelled">
          <div className="flex flex-wrap gap-3">
            {JOB_STATES.map((s) => (
              <JobStatusPill key={s} state={s} showLiteral />
            ))}
          </div>
        </GallerySwatch>
        <GallerySwatch label="Webhook attempt — pending / delivering / succeeded / failed (retrying) / dead">
          <div className="flex flex-wrap gap-3">
            {ATTEMPT_STATES.map((s) => (
              <AttemptStatusPill key={s} state={s} showLiteral />
            ))}
          </div>
        </GallerySwatch>
      </div>
    </Section>
  );
}
