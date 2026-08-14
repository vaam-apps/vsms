"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import {
  Badge,
  FormField,
  Input,
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
        <FormField label="Sender ID" htmlFor="gallery-input">
          <Input id="gallery-input" placeholder="VSMS-OTP" />
        </FormField>
        <FormField label="Environment" htmlFor="gallery-select">
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
        </FormField>
        <FormField
          label="Message body"
          htmlFor="gallery-textarea"
          className="col-span-1 sm:col-span-2"
        >
          <Textarea id="gallery-textarea" rows={3} placeholder="Votre code est 482913." />
        </FormField>
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
