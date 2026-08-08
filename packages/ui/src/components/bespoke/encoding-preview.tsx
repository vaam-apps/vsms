"use client";

import type { ReactNode } from "react";
import { cn } from "../../lib/cn";
import { Textarea } from "../primitives/textarea";

/**
 * Mirrors `compose.preview`'s expected shape (architecture doc §5.3's
 * `preview: PreviewMessageResult | null`). Defined locally rather than
 * imported — `@vsms/ui` has zero internal dependencies (T6 package rule),
 * so it cannot depend on `@vsms/api`'s generated types. This type is a
 * best-effort mirror pending the real tRPC procedure landing (T11); when
 * it does, that call site owns keeping the two in sync.
 */
export interface EncodingFlag {
  /** UTF-16 code-unit offset into `value`. */
  offset: number;
  length: number;
  char: string;
  reason: string;
  suggestion?: string;
}

export interface PreviewMessageResult {
  charset: "gsm7" | "ucs2";
  segments: number;
  charsUsed: number;
  charsPerSegment: number;
  flags: EncodingFlag[];
  transliteration?: {
    preview: string;
    charset: "gsm7" | "ucs2";
    segments: number;
  };
}

export interface EncodingPreviewProps {
  value: string;
  onChange: (value: string) => void;
  preview: PreviewMessageResult | null;
  isLoading?: boolean;
  transliterateEnabled?: boolean;
  onApplyTransliteration?: () => void;
  unitCostXaf?: number;
}

/**
 * Composer support component (#51) — makes `sms-encoding` visible to a
 * human. Deliberately a lighter build than the full design-doc spec (§5.3
 * names it, alongside `StateTimeline`/`LiveRow`, as one the data shape
 * hasn't settled for yet, so a correct-but-simpler stub is expected here):
 *
 * the design doc wants an absolutely-positioned highlight layer sitting
 * exactly behind the live-editable textarea, matching its font metrics
 * pixel-for-pixel — "a real implementation trap". Without a live composer
 * screen to verify that alignment against, this renders the flagged
 * characters in a separate, read-only annotated line below the editor
 * instead of an overlay. Same information, same character-level
 * precision, no risk of a silently-misaligned overlay. Swap in the overlay
 * once #51's real composer exists to verify metrics against.
 */
export function EncodingPreview({
  value,
  onChange,
  preview,
  isLoading = false,
  transliterateEnabled = false,
  onApplyTransliteration,
  unitCostXaf,
}: EncodingPreviewProps) {
  const isUcs2 = preview?.charset === "ucs2";
  const showRemedy =
    transliterateEnabled &&
    preview?.transliteration != null &&
    (preview.transliteration.charset !== preview.charset ||
      preview.transliteration.segments !== preview.segments);

  return (
    <div className="flex flex-col gap-2">
      <Textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={4}
        className="font-mono text-body"
        placeholder="Message body…"
      />

      {preview != null && preview.flags.length > 0 && (
        <p className="rounded-sm bg-base-100 px-2 py-1.5 font-mono text-[12px] text-subtle-foreground leading-relaxed">
          {renderAnnotated(value, preview.flags)}
        </p>
      )}

      <div
        className={cn("flex items-center gap-2 font-mono text-caption", isLoading && "opacity-60")}
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
            : `${preview.charsUsed}/${preview.charsPerSegment * preview.segments} chars`}
        </span>
        {unitCostXaf != null && preview != null && (
          <>
            <span className="text-muted-foreground">·</span>
            <span className="text-muted-foreground">~{unitCostXaf * preview.segments} FCFA</span>
          </>
        )}
      </div>

      {showRemedy && preview?.transliteration != null && (
        <div className="flex items-center justify-between gap-3 rounded-sm border border-edge bg-surface-2 px-3 py-2">
          <p className="text-caption text-muted-foreground">
            Transliteration would drop this to{" "}
            <span className="font-mono text-foreground">
              {preview.transliteration.charset.toUpperCase()} · {preview.transliteration.segments}{" "}
              seg
            </span>
            .
          </p>
          <button
            type="button"
            onClick={onApplyTransliteration}
            className="shrink-0 rounded-sm border border-edge-strong px-2 py-1 text-caption text-foreground hover:bg-surface-3"
          >
            Apply
          </button>
        </div>
      )}
    </div>
  );
}

function renderAnnotated(value: string, flags: EncodingFlag[]) {
  const sorted = [...flags].sort((a, b) => a.offset - b.offset);
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const flag of sorted) {
    if (flag.offset > cursor) parts.push(value.slice(cursor, flag.offset));
    const chunk = value.slice(flag.offset, flag.offset + flag.length);
    parts.push(
      <mark
        key={`${flag.offset}-${flag.length}`}
        title={flag.suggestion != null ? `${flag.reason} — try "${flag.suggestion}"` : flag.reason}
        className="rounded-[1px] bg-transparent text-state-uncertain-fg underline decoration-2 decoration-state-uncertain-fg underline-offset-2"
      >
        {chunk}
      </mark>,
    );
    cursor = flag.offset + flag.length;
  }
  if (cursor < value.length) parts.push(value.slice(cursor));
  return parts;
}
