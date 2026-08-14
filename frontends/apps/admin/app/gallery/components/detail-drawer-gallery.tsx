"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { Button, Input, MoreDetailDrawer, QuickDetailDrawer } from "@vsms/ui";
import { useState } from "react";
import { Section } from "./section";

export function DetailDrawerGallery() {
  const [quickOpen, setQuickOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);

  return (
    <Section
      title="Quick details vs. more details"
      description="Narrow/undimmed peek (§1.4, Mercury) vs. wide/dimmed destination (§1.5, Polar) — same vaul primitive, direction/width/dim baked in per variant so a call site can't blur the two."
    >
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="secondary" onClick={() => setQuickOpen(true)}>
          Open quick details
        </Button>
        <Button variant="secondary" onClick={() => setMoreOpen(true)}>
          Open more details
        </Button>
      </div>

      <QuickDetailDrawer
        open={quickOpen}
        onOpenChange={setQuickOpen}
        title="cs_msg_001"
        description="Quick details — a peek, not a destination."
        footer={
          <Button variant="ghost" size="sm" onClick={() => setQuickOpen(false)}>
            View full details
          </Button>
        }
      >
        <dl className="flex flex-col gap-3 text-body">
          <div className="flex justify-between gap-4">
            <dt className="text-muted-foreground">State</dt>
            <dd className="text-foreground">delivered</dd>
          </div>
          <div className="flex justify-between gap-4">
            <dt className="text-muted-foreground">Operator</dt>
            <dd className="text-foreground">mtn</dd>
          </div>
          <div className="flex justify-between gap-4">
            <dt className="text-muted-foreground">Segments</dt>
            <dd className="text-foreground">1</dd>
          </div>
        </dl>
      </QuickDetailDrawer>

      <MoreDetailDrawer
        open={moreOpen}
        onOpenChange={setMoreOpen}
        title="Provider: orange_cm"
        description="More details — the full record, edit form, destructive actions."
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => setMoreOpen(false)}>
              Cancel
            </Button>
            <Button variant="primary" size="sm" onClick={() => setMoreOpen(false)}>
              Save
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4 text-body">
          <p className="text-muted-foreground">
            A wide, dimmed drawer wide enough for a real edit form — this gallery entry stands in
            for what `providers-screen.tsx` will build in Phase 2.
          </p>
          <Input defaultValue="Orange Cameroon" aria-label="Display name" />
        </div>
      </MoreDetailDrawer>
    </Section>
  );
}
