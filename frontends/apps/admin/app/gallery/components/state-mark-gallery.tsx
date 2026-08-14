"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import {
  ATTEMPT_STATUS_META,
  JOB_STATUS_META,
  MESSAGE_STATUS_META,
  StateMarkFromMeta,
} from "@vsms/ui";
import { Section } from "./section";

/**
 * The raw eleven-glyph geometry (`StateMarkFromMeta`), independent of any
 * one state machine's label/hue — the design doc calls this "a correctness
 * artifact" (silhouette × interior mark × filled/knockout), worth its own
 * visual-QA row rather than only ever seen wrapped in a pill's own text.
 */
export function StateMarkGallery() {
  const allMeta = [
    ...Object.entries(MESSAGE_STATUS_META).map(([k, m]) => [`message:${k}`, m] as const),
    ...Object.entries(JOB_STATUS_META).map(([k, m]) => [`job:${k}`, m] as const),
    ...Object.entries(ATTEMPT_STATUS_META).map(([k, m]) => [`attempt:${k}`, m] as const),
  ];
  return (
    <Section
      title="State glyphs — raw geometry"
      description="StateMarkFromMeta, the primitive every status pill renders through. Silhouette (circle/diamond/square) × interior mark × filled-vs-knockout, at 16px."
    >
      <div className="flex flex-wrap gap-4">
        {allMeta.map(([key, meta]) => (
          <div key={key} className="flex flex-col items-center gap-1">
            <StateMarkFromMeta meta={meta} size={16} className="text-foreground" />
            <span className="font-mono text-[10px] text-subtle-foreground">{key}</span>
          </div>
        ))}
      </div>
    </Section>
  );
}
