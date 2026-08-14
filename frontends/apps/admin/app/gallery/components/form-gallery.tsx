"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import {
  Badge,
  Input,
  Label,
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Separator,
  Textarea,
} from "@vsms/ui";
import { Section } from "./section";

export function FormGallery() {
  return (
    <Section title="Inputs, textarea, select, label, badge, separator">
      <div className="grid max-w-xl grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gallery-input">Sender ID</Label>
          <Input id="gallery-input" placeholder="VSMS-OTP" />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gallery-select">Environment</Label>
          {/* SelectGroup: no visual treatment of its own (Radix's own
              SelectGroup didn't have one either) — mounted here purely so
              this gallery doesn't silently skip an export. */}
          <Select defaultValue="staging">
            <SelectTrigger id="gallery-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="staging">Staging</SelectItem>
                <SelectItem value="production">Production</SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
        <div className="col-span-1 flex flex-col gap-1.5 sm:col-span-2">
          <Label htmlFor="gallery-textarea">Message body</Label>
          <Textarea id="gallery-textarea" rows={3} placeholder="Votre code est 482913." />
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Badge>orange-cm</Badge>
        <Badge variant="outline">worker-2</Badge>
        <Separator orientation="vertical" className="h-4" />
        <Badge>staging</Badge>
      </div>
    </Section>
  );
}
