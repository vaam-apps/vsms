"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

// D18: `Tabs` was rebuilt as `ValueTabs` (Headless UI `TabGroup` behind a
// value-based adapter) — aliased on import so the JSX below is untouched.
import {
  ValueTabs as Tabs,
  ValueTabsContent as TabsContent,
  ValueTabsList as TabsList,
  ValueTabsTrigger as TabsTrigger,
} from "@vsms/ui";
import { Section } from "./section";

export function TabsGallery() {
  return (
    <Section title="Tabs" description="Underline variant only — no pill/segmented chrome.">
      <Tabs defaultValue="overview" className="max-w-md">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="timeline">Timeline</TabsTrigger>
          <TabsTrigger value="payloads">Payloads</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <p className="text-body text-muted-foreground">Message accepted at 14:03:07 UTC.</p>
        </TabsContent>
        <TabsContent value="timeline">
          <p className="text-body text-muted-foreground">See the State timeline section below.</p>
        </TabsContent>
        <TabsContent value="payloads">
          <p className="text-body text-muted-foreground">
            See the Payload inspector section below.
          </p>
        </TabsContent>
      </Tabs>
    </Section>
  );
}
