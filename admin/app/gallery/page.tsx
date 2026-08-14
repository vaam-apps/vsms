"use client";

import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
  CommandMenu,
  CommandMenuEmpty,
  CommandMenuGroup,
  CommandMenuInput,
  CommandMenuItem,
  CommandMenuList,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  EncodingPreview,
  InlineEmptyState,
  Input,
  Label,
  LiveRow,
  MESSAGE_STATES,
  type MessageState,
  MoreDetailDrawer,
  PayloadInspector,
  Popover,
  PopoverContent,
  PopoverTrigger,
  QuickDetailDrawer,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Separator,
  Skeleton,
  StateTimeline,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  Textarea,
  Toaster,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
  toast,
} from "@vsms/ui";
import { type ReactNode, useState } from "react";

const QUIET_STATES = MESSAGE_STATES.filter((s) =>
  ["accepted", "queued", "routed", "submitted", "delivered", "cancelled"].includes(s),
);
const LOUD_STATES = MESSAGE_STATES.filter((s) =>
  ["uncertain", "undelivered", "failed", "expired", "rejected"].includes(s),
);

function Section({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3">
      <div>
        <h2 className="font-medium text-foreground text-title-sm">{title}</h2>
        {description != null && (
          <p className="mt-1 text-caption text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}

function StatusPillGallery() {
  const [grayscale, setGrayscale] = useState(false);

  return (
    <Section
      title="Status system — eleven states"
      description="Every state messages_state_enum_check can produce, rendered in its natural (§4.5) attention treatment. delivered carries the owner's green-pill override; the rest of the ladder is unchanged."
    >
      <label className="flex w-fit items-center gap-2 text-caption text-muted-foreground">
        <input
          type="checkbox"
          checked={grayscale}
          onChange={(e) => setGrayscale(e.target.checked)}
          className="checkbox checkbox-sm"
        />
        Accessibility check: render at grayscale(1) — all eleven must stay distinguishable (§4.6)
      </label>
      <div className={grayscale ? "grayscale" : undefined}>
        <div className="flex flex-col gap-4">
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Quiet — on track / uneventful terminal
            </p>
            <div className="flex flex-wrap gap-3">
              {QUIET_STATES.map((s) => (
                <StatusPill key={s} state={s} showLiteral />
              ))}
            </div>
          </div>
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Loud — needs a human
            </p>
            <div className="flex flex-wrap gap-3">
              {LOUD_STATES.map((s) => (
                <StatusPill key={s} state={s} showLiteral />
              ))}
            </div>
          </div>
        </div>
      </div>
    </Section>
  );
}

function ButtonGallery() {
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
        <Button variant="secondary" disabled>
          Disabled
        </Button>
      </div>
    </Section>
  );
}

function FormGallery() {
  return (
    <Section title="Inputs, textarea, select, label, badge, separator">
      <div className="grid max-w-xl grid-cols-2 gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gallery-input">Sender ID</Label>
          <Input id="gallery-input" placeholder="VSMS-OTP" />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="gallery-select">Environment</Label>
          <Select defaultValue="staging">
            <SelectTrigger id="gallery-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="staging">Staging</SelectItem>
              <SelectItem value="production">Production</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="col-span-2 flex flex-col gap-1.5">
          <Label htmlFor="gallery-textarea">Message body</Label>
          <Textarea id="gallery-textarea" rows={3} placeholder="Votre code est 482913." />
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Badge>orange-cm</Badge>
        <Badge variant="outline">worker-2</Badge>
        <Separator orientation="vertical" className="h-4" />
        <Badge>staging</Badge>
      </div>
    </Section>
  );
}

const DEMO_ROWS: Array<{ id: string; state: MessageState; recipient: string; version: number }> = [
  { id: "cs_msg_001", state: "delivered", recipient: "+237 6 77 12 34 56", version: 3 },
  { id: "cs_msg_002", state: "uncertain", recipient: "+237 6 91 22 10 09", version: 2 },
  { id: "cs_msg_003", state: "queued", recipient: "+237 6 55 40 18 77", version: 1 },
];

function TableGallery() {
  const [tick, setTick] = useState(0);
  return (
    <Section
      title="Table + LiveRow"
      description="Status column first (§6.4). Click the button to trigger a 240ms wash on the first row, as if its state had just changed — nothing else in the row moves."
    >
      <Button variant="secondary" size="sm" onClick={() => setTick((t) => t + 1)}>
        Simulate a state change on row 1
      </Button>
      <Card>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Status</TableHead>
              <TableHead>Recipient</TableHead>
              <TableHead>Id</TableHead>
              <TableHead align="end">Version</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {DEMO_ROWS.map((row, i) => (
              <LiveRow key={row.id} washTrigger={i === 0 ? tick : row.version} washHue="success">
                <TableCell>
                  <StatusPill state={row.state} />
                </TableCell>
                <TableCell mono>{row.recipient}</TableCell>
                <TableCell mono>{row.id}</TableCell>
                <TableCell align="end" mono>
                  {row.version}
                </TableCell>
              </LiveRow>
            ))}
          </TableBody>
        </Table>
      </Card>
      <InlineEmptyState
        message="No webhook attempts match the current filters."
        action={{ label: "Clear filters", onClick: () => {} }}
      />
      <div className="flex flex-col gap-1">
        <p className="text-caption text-muted-foreground">Loading skeleton (static, no shimmer):</p>
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-10 w-full" />
      </div>
    </Section>
  );
}

function TabsGallery() {
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

function OverlaysGallery() {
  return (
    <Section
      title="Dialog, dropdown menu, tooltip, popover, drawer, command menu, toast"
      description="Radix behaviour (focus trap, keyboard nav, ARIA) under daisyUI styling."
    >
      <div className="flex flex-wrap items-center gap-3">
        <Dialog>
          <DialogTrigger asChild>
            <Button variant="secondary">Open dialog</Button>
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Cancel message?</DialogTitle>
              <DialogDescription>
                This proposes a cancellation to the API — Postgres still decides.
              </DialogDescription>
            </DialogHeader>
            <div className="flex justify-end gap-2">
              <Button variant="destructive" size="sm">
                Cancel message
              </Button>
            </div>
          </DialogContent>
        </Dialog>

        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="secondary">Row actions</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuLabel>cs_msg_001</DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem>Copy id</DropdownMenuItem>
            <DropdownMenuItem>Open detail</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="secondary">Hover me</Button>
            </TooltipTrigger>
            <TooltipContent>Inferred from prefix — not authoritative.</TooltipContent>
          </Tooltip>
        </TooltipProvider>

        <Popover>
          <PopoverTrigger asChild>
            <Button variant="secondary">Open popover</Button>
          </PopoverTrigger>
          <PopoverContent>
            <p className="text-body text-foreground">ç — LATIN SMALL LETTER C WITH CEDILLA</p>
            <p className="mt-1 text-caption text-muted-foreground">
              U+00E7. Forces UCS-2. Try "c" instead.
            </p>
          </PopoverContent>
        </Popover>

        <Drawer direction="right">
          <DrawerTrigger asChild>
            <Button variant="secondary">Open drawer</Button>
          </DrawerTrigger>
          <DrawerContent>
            <div className="flex items-center justify-between p-4">
              <p className="font-medium text-foreground text-title-sm">cs_msg_001</p>
              <DrawerClose asChild>
                <Button variant="ghost" size="sm">
                  Close
                </Button>
              </DrawerClose>
            </div>
          </DrawerContent>
        </Drawer>

        <Button
          variant="secondary"
          onClick={() =>
            toast({
              title: "Copied",
              description: "cs_msg_001 copied to clipboard.",
              variant: "success",
            })
          }
        >
          Fire a toast
        </Button>
      </div>

      <CommandMenu className="max-w-md">
        <CommandMenuInput placeholder="Search messages, apps, routes…" />
        <CommandMenuList>
          <CommandMenuEmpty>No results.</CommandMenuEmpty>
          <CommandMenuGroup heading="Recent">
            <CommandMenuItem>cs_msg_001 — delivered</CommandMenuItem>
            <CommandMenuItem>cs_msg_002 — uncertain</CommandMenuItem>
          </CommandMenuGroup>
        </CommandMenuList>
      </CommandMenu>
    </Section>
  );
}

// console-redesign.md §3/D14: the two baked-direction, baked-dim drawer
// variants Phase 2's "Delivery" agent will build every Provider/Route/
// Sender ID/Webhook quick-vs-more pair on top of. This is the QA surface
// for both — resize the browser pane to check the phone/desktop split
// (base = bottom sheet, `md`+ = right panel) and confirm quick details
// never dims while more details does.
function DetailDrawerGallery() {
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

function PayloadInspectorGallery() {
  return (
    <Section title="Payload inspector">
      <PayloadInspector
        exchanges={[
          {
            direction: "request",
            method: "POST",
            url: "https://api.orange.cm/smsmessaging/v1/outbound/tel:+237.../requests",
            status: 201,
            durationMs: 214,
            headers: { "content-type": "application/json" },
            body: '{\n  "outboundSMSMessageRequest": {\n    "address": "tel:+237677123456",\n    "senderAddress": "tel:VSMS-OTP"\n  }\n}',
          },
          {
            direction: "callback",
            method: "POST",
            url: "/dlr/orange-cm",
            status: 200,
            durationMs: 4,
            body: '{"deliveryInfo":{"deliveryStatus":"DeliveredToTerminal"}}',
          },
        ]}
      />
    </Section>
  );
}

function StateTimelineGallery() {
  return (
    <Section
      title="State timeline — the epic gate"
      description="Diagnose a message without touching SQL. The annotation nodes carry the §4.7 copy for uncertain/undelivered verbatim."
    >
      <Card>
        <CardHeader title="cs_msg_002" meta="+237 6 91 22 10 09 · MTN" />
        <CardBody>
          <StateTimeline
            currentState="uncertain"
            isTerminal={false}
            transitions={[
              { toState: "accepted", at: "2026-08-08T14:03:07.412Z", actor: "app:vsms-console" },
              { toState: "queued", at: "2026-08-08T14:03:07.690Z" },
              { toState: "routed", at: "2026-08-08T14:03:08.010Z", providerKey: "orange-cm" },
              {
                toState: "submitted",
                at: "2026-08-08T14:03:08.312Z",
                providerKey: "orange-cm",
                workerNode: "worker-2",
                attempt: 1,
                maxAttempts: 3,
              },
              {
                toState: "uncertain",
                at: "2026-08-08T14:03:38.312Z",
                providerKey: "orange-cm",
                workerNode: "worker-2",
                attempt: 1,
                maxAttempts: 3,
              },
            ]}
          />
        </CardBody>
      </Card>
    </Section>
  );
}

function EncodingPreviewGallery() {
  const [value, setValue] = useState("Votre code a été reçu");
  const hasCedilla = value.includes("ç");
  return (
    <Section
      title="Encoding preview"
      description="Composer support component (#51) — makes sms-encoding visible before 50,000 sends. Real shape: encoding/segments/length/perSegment/offending (distinct characters, not offsets)/suggestion — see the composer at / for the live version wired to compose.preview."
    >
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
    </Section>
  );
}

export default function GalleryPage() {
  return (
    <TooltipProvider>
      <main className="mx-auto flex max-w-5xl flex-col gap-10 px-6 py-10">
        <header className="flex items-start justify-between gap-4 border-edge border-b pb-6">
          <div>
            <p className="font-mono text-micro text-subtle-foreground tracking-[0.03em]">
              @vsms/ui — T6
            </p>
            <h1 className="mt-1 font-medium text-foreground text-title">Component gallery</h1>
            <p className="mt-1 max-w-2xl text-body text-muted-foreground">
              An honest rendering of the status system and every primitive — not a fake dashboard.
              This page's only job is to prove the design tokens, daisyUI theming, and behaviour
              actually work. Dark-only (D9) — there is no second theme to switch to.
            </p>
          </div>
        </header>

        <StatusPillGallery />
        <Separator />
        <ButtonGallery />
        <Separator />
        <FormGallery />
        <Separator />
        <TableGallery />
        <Separator />
        <TabsGallery />
        <Separator />
        <OverlaysGallery />
        <Separator />
        <DetailDrawerGallery />
        <Separator />
        <PayloadInspectorGallery />
        <Separator />
        <StateTimelineGallery />
        <Separator />
        <EncodingPreviewGallery />
      </main>
      <Toaster />
    </TooltipProvider>
  );
}
