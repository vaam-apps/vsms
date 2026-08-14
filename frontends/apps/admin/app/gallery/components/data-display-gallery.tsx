"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { IdDisplay, MsisdnDisplay, TimestampDisplay } from "@vsms/ui";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

export function DataDisplayGallery() {
  const longId = "cs_a1b2c3d4e5f6g7h8i9j0k1l2";
  return (
    <Section
      title="Id, MSISDN, and timestamp display"
      description="Design doc §7.1–§7.3: never truncate an MSISDN, never middle-ellipsis an id, never show a bare local time with no zone. Not previously mounted anywhere in this gallery — a real coverage gap, closed in this pass."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="IdDisplay — table variant (7 chars + hover-reveal copy) vs. full (complete, selectable)">
          <div className="flex flex-wrap items-center gap-6">
            <IdDisplay value={longId} />
            <IdDisplay value={longId} variant="full" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="MsisdnDisplay — with and without a known operator, and an unrecognised shape (falls back to the raw string rather than mis-grouping it)">
          <div className="flex flex-wrap items-center gap-6">
            <MsisdnDisplay value="+237677123456" operator="mtn" />
            <MsisdnDisplay value="+237655123456" operator="orange" />
            <MsisdnDisplay value="+237677123456" />
            <MsisdnDisplay value="not-a-real-msisdn" />
          </div>
        </GallerySwatch>
        <GallerySwatch
          label={
            <>
              TimestampDisplay — under 24h renders relative (mono, hover for absolute), older falls
              back to the absolute ISO-ordered UTC form. Fixed literal timestamps, not computed from
              `Date.now()` at render time — that computes a different value on the server than on
              the client's own hydration pass (a real hydration-mismatch bug this pass found and
              fixed: React's own "server rendered text didn't match the client" error, reproduced
              live in the console before this fix). `TimestampDisplay`'s own component is fine — it
              already renders the identical absolute string on both passes and only upgrades to
              relative after mounting; the bug was in this gallery's own inline `Date.now()` call,
              not in `@vsms/ui`.
            </>
          }
        >
          <div className="flex flex-wrap items-center gap-6">
            <TimestampDisplay value="2026-08-14T15:57:00Z" />
            <TimestampDisplay value="2026-08-14T12:57:00Z" />
            <TimestampDisplay value="2026-01-04T09:12:31Z" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Overflow check — a long id inside a narrow (200px) container">
          <div className="w-[200px] rounded-sm border border-edge bg-surface-2 p-2">
            <IdDisplay value={longId} variant="full" />
          </div>
        </GallerySwatch>
      </div>
    </Section>
  );
}
