"use client";

import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Button } from "../primitives/button";
import { Textarea } from "../primitives/textarea";

/**
 * Mirrors `@vsms/gateway`'s real `PreviewResult` (`frontends/packages/gateway/src/client.ts`,
 * transcribed from `schemas/vsms.cstack`'s `PreviewResult` type, verified
 * live against `backends/crates/sms-api/src/procedures.rs::preview`). Defined
 * locally, as a deliberate subset, rather than imported — `@vsms/ui` has
 * zero internal dependencies (T6 package rule), so it cannot depend on
 * `@vsms/gateway`'s types. Callers may pass the full `PreviewResult`
 * straight through; TypeScript's structural typing accepts the extra
 * `operator`/`normalizedTo` fields it doesn't use.
 *
 * **`offending` is an array of the offending CHARACTERS, not byte or
 * codepoint offsets.** An earlier revision of this component (and the
 * architecture plan it was drafted from) assumed per-occurrence
 * `{ offset, length }` flags — that was written before anyone read the
 * real wire type. `backends/crates/sms-api/src/procedures.rs::distinct_offending`
 * collapses every occurrence to its first appearance, so twenty copies of
 * `ç` arrive as one entry. Highlighting therefore matches characters
 * against `value` directly (below), not positions.
 */
export interface EncodingPreviewResult {
  encoding: "gsm7" | "ucs2";
  segments: number;
  length: number;
  perSegment: number;
  /** Distinct offending characters, first-occurrence order. */
  offending: string[];
  /** The transliterated body — present only when the body is UCS-2 *and*
   * transliterating it would actually land it back in GSM-7
   * (`sms_encoding::analyse`'s own doc: "`Some` only when ... transliteration
   * would actually rescue it"). Applying it does not predict the resulting
   * segment count here; the natural debounced re-preview after the body
   * changes shows the real new numbers rather than this component guessing
   * them. `| undefined` explicit, not just `?:` — `tsconfig.base.json`'s
   * `exactOptionalPropertyTypes` would otherwise reject assigning the real
   * `PreviewResult` (whose own `suggestion` is typed the same explicit way,
   * per `@vsms/gateway/client.ts`'s own module doc) straight into this
   * prop. */
  suggestion?: string | undefined;
}

export interface EncodingPreviewProps {
  value: string;
  onChange: (value: string) => void;
  onBlur?: () => void;
  /** Forwarded to the underlying `<textarea>` for label association
   * (`<Label htmlFor>`) and `react-hook-form`'s `Controller` wiring. */
  id?: string;
  name?: string;
  "aria-invalid"?: boolean;
  preview: EncodingPreviewResult | null;
  /** A preview request is in flight. Dims the stat line rather than
   * clearing it — a flicker on live-typing feedback is worse than a
   * 200ms-stale number (design doc §5.3). */
  isLoading?: boolean;
  /** The last preview request failed (most often: `to` doesn't parse as a
   * recipient yet). `preview` still holds the last successful result;
   * this only marks it as no longer current. */
  isStale?: boolean;
  onApplySuggestion?: () => void;
  className?: string;
}

/**
 * The composer's (#51) encoding surface — makes `sms-encoding` visible to a
 * human before a `ç` silently doubles a send's cost. Three bands: the
 * editor, an annotated read-only line calling out offending characters,
 * and a status line (charset · segments · units).
 *
 * Deliberately lighter than the design doc's full `EncodingPreview` spec in
 * one respect, kept from the component's original build: the design doc
 * wants an absolutely-positioned highlight layer sitting pixel-for-pixel
 * behind the live-editable textarea. That alignment is "a real
 * implementation trap" per the doc's own words, and a misaligned overlay is
 * worse than no overlay — a separate annotated line below the editor
 * carries the same character-level information with no alignment risk.
 */
export function EncodingPreview({
  value,
  onChange,
  onBlur,
  id,
  name,
  "aria-invalid": ariaInvalid,
  preview,
  isLoading = false,
  isStale = false,
  onApplySuggestion,
  className,
}: EncodingPreviewProps) {
  const isUcs2 = preview?.encoding === "ucs2";
  const offendingSet = new Set(preview?.offending ?? []);
  const dimmed = isLoading || isStale;

  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <Textarea
        id={id}
        name={name}
        aria-invalid={ariaInvalid}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onBlur={onBlur}
        rows={5}
        className="font-mono text-body"
        placeholder="Message body…"
      />

      {offendingSet.size > 0 && (
        <p className="rounded-sm bg-base-100 px-2 py-1.5 font-mono text-[12px] text-subtle-foreground leading-relaxed">
          {renderAnnotated(value, offendingSet)}
        </p>
      )}

      <div
        className={cn(
          "flex flex-wrap items-center gap-2 font-mono text-caption",
          dimmed && "opacity-60",
        )}
      >
        <span
          className={cn(
            "rounded-sm px-1.5 py-0.5",
            isUcs2
              ? "border border-state-uncertain-border bg-state-uncertain-bg text-state-uncertain-fg"
              : "text-muted-foreground",
          )}
        >
          {preview == null ? "—" : isUcs2 ? "UCS-2" : "GSM-7"}
        </span>
        <span className="text-muted-foreground">
          {preview == null
            ? "—"
            : `${preview.segments} segment${preview.segments === 1 ? "" : "s"}`}
        </span>
        <span className="text-muted-foreground">·</span>
        <span className="text-muted-foreground">
          {preview == null
            ? "—"
            : `${preview.length}/${preview.perSegment * preview.segments} chars`}
        </span>
        {isStale && <span className="text-state-uncertain-fg">· stale</span>}
      </div>

      {offendingSet.size > 0 && (
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-caption text-subtle-foreground">Not in GSM-7:</span>
          {[...offendingSet].map((ch) => (
            <span
              key={ch}
              title={`U+${(ch.codePointAt(0) ?? 0)
                .toString(16)
                .toUpperCase()
                .padStart(4, "0")} — not in the GSM-7 default alphabet, forces UCS-2`}
              className="rounded-sm border border-state-uncertain-border bg-state-uncertain-bg px-1.5 py-0.5 font-mono text-[12px] text-state-uncertain-fg"
            >
              {ch}
            </span>
          ))}
        </div>
      )}

      {preview?.suggestion != null && (
        <div className="flex items-center justify-between gap-3 rounded-sm border border-edge bg-surface-2 px-3 py-2">
          <p className="text-caption text-muted-foreground">
            Removing accents would fit this in{" "}
            <span className="font-mono text-foreground">GSM-7</span>.
          </p>
          <Button type="button" variant="secondary" size="sm" onClick={onApplySuggestion}>
            Apply
          </Button>
        </div>
      )}
    </div>
  );
}

/** Wraps every occurrence of a character in `offending` in a `<mark>`,
 * iterating `value` by Unicode code point (`for...of` on a string, not
 * index access) so a character outside the BMP is never split across a
 * surrogate pair. */
function renderAnnotated(value: string, offending: Set<string>): ReactNode[] {
  const parts: ReactNode[] = [];
  let buffer = "";
  let key = 0;

  const flush = () => {
    if (buffer !== "") {
      parts.push(buffer);
      buffer = "";
    }
  };

  for (const ch of value) {
    if (offending.has(ch)) {
      flush();
      parts.push(
        <mark
          key={`${key++}-${ch}`}
          className="rounded-[1px] bg-transparent text-state-uncertain-fg underline decoration-2 decoration-state-uncertain-fg underline-offset-2"
        >
          {ch}
        </mark>,
      );
    } else {
      buffer += ch;
    }
  }
  flush();
  return parts;
}
