"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { EncodingPreview } from "@vsms/ui";
import { useState } from "react";
import { Section } from "./section";

export function EncodingPreviewGallery() {
  const [value, setValue] = useState("Votre code a été reçu");
  const hasCedilla = value.includes("ç");
  return (
    <Section
      title="Encoding preview"
      description="Composer support component (#51) — makes sms-encoding visible before 50,000 sends. Real shape: encoding/segments/length/perSegment/offending (distinct characters, not offsets)/suggestion — see the composer at / for the live version wired to compose.preview. Second instance below is the isLoading state (a request in flight, per its own doc: dims the stat line rather than clearing it)."
    >
      <div className="flex flex-col gap-4">
        <div className="max-w-xl">
          <EncodingPreview
            value={value}
            onChange={setValue}
            isLoading={false}
            onApplySuggestion={() => setValue((v) => v.replace(/ç/g, "c"))}
            preview={{
              encoding: hasCedilla ? "ucs2" : "gsm7",
              segments: 1,
              length: value.length,
              perSegment: hasCedilla ? 70 : 160,
              offending: hasCedilla ? ["ç"] : [],
              ...(hasCedilla ? { suggestion: value.replace(/ç/g, "c") } : {}),
            }}
          />
        </div>
        <div className="max-w-xl">
          <p className="mb-1 text-caption text-muted-foreground">
            isLoading (a request in flight):
          </p>
          <EncodingPreview
            value="Un nouveau message en cours de frappe"
            onChange={() => {}}
            isLoading
            preview={{
              encoding: "ucs2",
              segments: 1,
              length: 38,
              perSegment: 70,
              offending: ["é"],
            }}
          />
        </div>
      </div>
    </Section>
  );
}
