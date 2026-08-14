"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { Button, buttonVariants } from "@vsms/ui";
import { Section } from "./section";

export function ButtonGallery() {
  return (
    <Section
      title="Button"
      description="daisyUI's btn class; no success/warning variant on purpose — those hues are status-only (§1.3)."
    >
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="primary">Primary</Button>
        <Button variant="secondary">Secondary</Button>
        <Button variant="ghost">Ghost</Button>
        <Button variant="destructive">Destructive</Button>
        <Button variant="primary" size="sm">
          Small
        </Button>
        <Button variant="secondary" size="icon" aria-label="Icon-only">
          ⋯
        </Button>
        <Button variant="secondary" disabled>
          Disabled
        </Button>
        {/* D4/D11: buttonVariants exported standalone, no Slot/asChild — a
            link that must look like a button reaches for the class string
            directly rather than a polymorphic wrapper. */}
        <a href="/dashboard" className={buttonVariants({ variant: "secondary", size: "sm" })}>
          Link styled as button
        </a>
      </div>
    </Section>
  );
}
