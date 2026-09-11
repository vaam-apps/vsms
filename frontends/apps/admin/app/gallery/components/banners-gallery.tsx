"use client";

// Route-local (R6). Closes a real coverage gap rather than adding a new
// demo for its own sake: this page's own doc says it "imports and
// exercises every `@vaam-apps/ui` export, so it doubles as the console's
// own visual-QA surface. A gallery that silently drops an export is a QA
// surface with a blind spot." An audit of the package's 102 component
// exports against this directory found 13 that nothing here mounted —
// among them `StateChip`, `StaleWriteBanner` and `FieldError`.
//
// That is not an abstract tidiness point. Those three are exactly the
// surfaces that carried the two live bugs found while generalising the
// package for release: `--state-warning-*` was referenced by
// `StateChip tone="warning"` and by every `StaleWriteBanner` but never
// declared, so both rendered with no colour at all; and `cn()` was
// silently deleting the font size out of `FieldError`'s own
// `text-caption text-state-danger-fg`. Both survived because nothing
// rendered them side by side with their siblings, where "this one is
// colourless" and "this one is the wrong size" are obvious at a glance.

import {
  ChipSelect,
  Code,
  FieldError,
  FormField,
  InlineBanner,
  InlineEmptyState,
  Input,
  Label,
  PhoneDisplay,
  RadioGroup,
  StaleWriteBanner,
  StateChip,
  type StateChipTone,
  toast,
} from "@vaam-apps/ui";
import { useState } from "react";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

const BANNER_VARIANTS = ["neutral", "danger", "warning", "success", "uncertain", "plain"] as const;

const CHIP_TONES: StateChipTone[] = [
  "neutral",
  "success",
  "warning",
  "danger",
  "uncertain",
  "expired",
  "parked",
];

export function BannersGallery() {
  const [classes, setClasses] = useState<string[]>(["otp"]);
  const [radio, setRadio] = useState<"approved" | "rejected">("approved");

  return (
    <Section
      title="Banners, chips, empty states, and field errors"
      description="Every variant of each, side by side — the arrangement that makes a missing colour token or a dropped font size obvious. Both of those were real bugs in this package, and both survived precisely because nothing rendered these together."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="InlineBanner — all six variants. `warning` referenced --state-warning-* tokens that were never declared until recently; it rendered as plain grey text.">
          <div className="flex flex-col gap-2">
            {BANNER_VARIANTS.map((variant) => (
              <InlineBanner key={variant} variant={variant}>
                {variant} — the quick brown fox jumps over the lazy dog
              </InlineBanner>
            ))}
          </div>
        </GallerySwatch>
        <GallerySwatch label="StateChip — all seven hues of the shared StatusHue vocabulary. It used to carry its own four-tone copy, so expired and parked were unreachable.">
          <div className="flex flex-wrap items-center gap-2">
            {CHIP_TONES.map((tone) => (
              <StateChip key={tone} tone={tone}>
                {tone}
              </StateChip>
            ))}
          </div>
        </GallerySwatch>
        <GallerySwatch label="StaleWriteBanner — the 412 Precondition Failed notice. warning, not danger: nothing was lost, the write was refused so the other operator's edit survives.">
          <StaleWriteBanner
            onReload={() => toast({ title: "Pretend reload", variant: "default" })}
          />
        </GallerySwatch>
        <GallerySwatch label="FieldError and FormField — the error line is text-caption (12px). If it renders at the browser default, cn() has lost the font size to the colour beside it again.">
          <div className="flex max-w-sm flex-col gap-3">
            <FormField
              label="Sender ID"
              htmlFor="gallery-sender"
              hint="Three to eleven characters, alphanumeric."
              error="Must be at most 11 characters."
            >
              <Input id="gallery-sender" defaultValue="A-SENDER-ID-THAT-IS-TOO-LONG" />
            </FormField>
            <FieldError>A standalone FieldError, outside any FormField.</FieldError>
          </div>
        </GallerySwatch>
        <GallerySwatch label="InlineEmptyState — an inline status line, never a centred placard with an illustration">
          <div className="flex flex-col gap-2 rounded-sm border border-edge bg-surface-2 px-3">
            <InlineEmptyState message="No messages match these filters." />
            <InlineEmptyState
              message="No routes configured."
              action={{
                label: "Seed a catch-all",
                onClick: () => toast({ title: "Pretend seeded" }),
              }}
            />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Label, Code, and PhoneDisplay — PhoneDisplay with a caller-supplied formatter and tag, and with neither (raw E.164, em-dash tag)">
          <div className="flex flex-col gap-3">
            <Label htmlFor="gallery-nothing">A standalone Label</Label>
            <p className="text-body text-muted-foreground">
              A role key rendered inline: <Code>operator</Code>, and a job kind:{" "}
              <Code>expire_stale</Code>.
            </p>
            <div className="flex flex-wrap items-center gap-6">
              <PhoneDisplay
                value="+237677123456"
                format={(v) =>
                  `${v.slice(0, 4)} ${v.slice(4, 5)} ${v.slice(5).replace(/(..)/g, "$1 ").trim()}`
                }
                tag="MTN"
                tagTitle="Inferred from prefix — not authoritative."
              />
              <PhoneDisplay value="+14155550132" />
            </div>
          </div>
        </GallerySwatch>
        <GallerySwatch label="ChipSelect and RadioGroup — the two small-vocabulary controls. Neither portals, so both are safe inside a drawer.">
          <div className="flex flex-col gap-4">
            <ChipSelect
              options={[
                { value: "otp", label: "OTP" },
                { value: "transactional", label: "Transactional" },
                { value: "notification", label: "Notification" },
                { value: "marketing", label: "Marketing" },
              ]}
              value={classes}
              onValueChange={setClasses}
              aria-label="Message class"
            />
            <RadioGroup
              value={radio}
              onValueChange={setRadio}
              aria-label="Registration decision"
              options={[
                { value: "approved", label: "Approve", description: "The provider accepted it." },
                { value: "rejected", label: "Reject", description: "Needs a reason." },
              ]}
            />
          </div>
        </GallerySwatch>
      </div>
    </Section>
  );
}
