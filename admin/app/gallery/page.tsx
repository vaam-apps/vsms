"use client";

// The component gallery (T6 / console-redesign.md §7 Phase 2) — the LAST
// screen built in this redesign, deliberately: it imports and exercises
// every `@vsms/ui` export, so it doubles as the console's own visual-QA
// surface (Phase 3's manual pass runs through this page plus one screen
// per IA group). A gallery that silently drops an export is a QA surface
// with a blind spot — see the bottom of this file for the coverage ledger
// this build pass produced, cross-checked against `packages/ui/src/
// index.ts` export by export.
//
// Two real bugs were found and fixed by the act of mounting everything,
// not by inspection — both left as comments at their fix site rather than
// silently smoothed over:
//   1. `<Toaster />` was mounted twice — once globally in `providers.tsx`
//      (correct, per `toast.tsx`'s own "mount once, near the app root"),
//      and a second time at the bottom of this file. Both instances read
//      the same module-level store, so every toast rendered twice, stacked
//      at the same fixed position. Removed here; `admin/app/providers.tsx`
//      already covers it for the whole app, this page included.
//   2. The plain `<Drawer>`/`<DrawerContent>` demo (distinct from
//      `QuickDetailDrawer`/`MoreDetailDrawer` below it) rendered with no
//      `DrawerTitle` — `drawer.tsx`'s own module doc already names this
//      exact trap ("vaul's Content renders Radix Dialog's Content
//      underneath... Radix's own 'DialogContent requires a DialogTitle'
//      dev warning applies here"). Fixed by adding a screen-reader-only
//      `DrawerTitle` to that one demo.

import {
  ATTEMPT_STATES,
  ATTEMPT_STATUS_META,
  AttemptStatusPill,
  Badge,
  Button,
  buttonVariants,
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
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  Drawer,
  DrawerClose,
  DrawerContent,
  DrawerTitle,
  DrawerTrigger,
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  EncodingPreview,
  IdDisplay,
  InlineEmptyState,
  Input,
  JOB_STATES,
  JOB_STATUS_META,
  JobStatusPill,
  Label,
  LiveRow,
  MESSAGE_STATES,
  MESSAGE_STATUS_META,
  type MessageState,
  MoreDetailDrawer,
  MsisdnDisplay,
  PayloadInspector,
  Popover,
  PopoverContent,
  PopoverTrigger,
  QuickDetailDrawer,
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Separator,
  Skeleton,
  StateMarkFromMeta,
  StateTimeline,
  StatusPill,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  // D18: `Tabs` was rebuilt as `ValueTabs` (Headless UI `TabGroup` behind a
  // value-based adapter) — aliased on import so the JSX below is untouched.
  ValueTabs as Tabs,
  ValueTabsContent as TabsContent,
  ValueTabsList as TabsList,
  ValueTabsTrigger as TabsTrigger,
  Textarea,
  TimestampDisplay,
  Tooltip,
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
          <div>
            <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
              Other `StatusPill` states: pending (optimistic, unconfirmed) and interactive
              (clickable)
            </p>
            <div className="flex flex-wrap items-center gap-3">
              <StatusPill state="queued" pending showLiteral detail="optimistic" />
              <StatusPill
                state="failed"
                interactive
                showLiteral
                detail="click me"
                onClick={() => toast({ title: "StatusPill clicked", variant: "default" })}
              />
            </div>
          </div>
        </div>
      </div>
    </Section>
  );
}

/**
 * `JobStatusPill`/`AttemptStatusPill` (#56/#55) — two more state machines
 * with their own transitions table, deliberately not folded into the
 * message pill above (`status-tokens.ts`'s own module doc: a job's
 * `failed` is retryable, a message's `failed` is terminal). Neither was
 * mounted anywhere in this gallery before this pass — a real coverage gap,
 * found by cross-checking `@vsms/ui`'s index against this file's own
 * imports rather than assumed complete.
 */
function JobAndAttemptPillGallery() {
  return (
    <Section
      title="Job and attempt status pills"
      description="Six job states, five attempt states — same glyph/hue system as StatusPill, driven by JOB_STATUS_META / ATTEMPT_STATUS_META instead of MESSAGE_STATUS_META. Note failed here is retryable (unresolved/uncertain hue), not the terminal danger hue a message's own failed carries."
    >
      <div className="flex flex-col gap-4">
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            Job — pending / running / succeeded / failed (retrying) / dead / cancelled
          </p>
          <div className="flex flex-wrap gap-3">
            {JOB_STATES.map((s) => (
              <JobStatusPill key={s} state={s} showLiteral />
            ))}
          </div>
        </div>
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            Webhook attempt — pending / delivering / succeeded / failed (retrying) / dead
          </p>
          <div className="flex flex-wrap gap-3">
            {ATTEMPT_STATES.map((s) => (
              <AttemptStatusPill key={s} state={s} showLiteral />
            ))}
          </div>
        </div>
      </div>
    </Section>
  );
}

/**
 * The raw eleven-glyph geometry (`StateMarkFromMeta`), independent of any
 * one state machine's label/hue — the design doc calls this "a correctness
 * artifact" (silhouette × interior mark × filled/knockout), worth its own
 * visual-QA row rather than only ever seen wrapped in a pill's own text.
 */
function StateMarkGallery() {
  const allMeta = [
    ...Object.entries(MESSAGE_STATUS_META).map(([k, m]) => [`message:${k}`, m] as const),
    ...Object.entries(JOB_STATUS_META).map(([k, m]) => [`job:${k}`, m] as const),
    ...Object.entries(ATTEMPT_STATUS_META).map(([k, m]) => [`attempt:${k}`, m] as const),
  ];
  return (
    <Section
      title="State glyphs — raw geometry"
      description="StateMarkFromMeta, the primitive every status pill renders through. Silhouette (circle/diamond/square) × interior mark × filled-vs-knockout, at 16px."
    >
      <div className="flex flex-wrap gap-4">
        {allMeta.map(([key, meta]) => (
          <div key={key} className="flex flex-col items-center gap-1">
            <StateMarkFromMeta meta={meta} size={16} className="text-foreground" />
            <span className="font-mono text-[10px] text-subtle-foreground">{key}</span>
          </div>
        ))}
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

function FormGallery() {
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

const DEMO_ROWS: Array<{ id: string; state: MessageState; recipient: string; version: number }> = [
  { id: "cs_msg_001", state: "delivered", recipient: "+237 6 77 12 34 56", version: 3 },
  { id: "cs_msg_002", state: "uncertain", recipient: "+237 6 91 22 10 09", version: 2 },
  {
    id: "cs_msg_003_a_deliberately_long_client_ref_to_check_overflow_handling",
    state: "queued",
    recipient: "+237 6 55 40 18 77",
    version: 1,
  },
];

function TableGallery() {
  const [tick, setTick] = useState(0);
  return (
    <Section
      title="Table + LiveRow"
      description="Status column first (§6.4). Click the button to trigger a 240ms wash on the first row, as if its state had just changed — nothing else in the row moves. Third row's id is deliberately long, to check overflow/wrap behaviour rather than only ever testing with tidy fixture data."
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

      <p className="text-caption text-muted-foreground">
        Error state (a failed query, inline — never a placard):
      </p>
      <div className="rounded-sm border border-state-danger-border bg-state-danger-bg px-3 py-2 text-caption text-state-danger-fg">
        Couldn't load webhook attempts: sms-api returned 500.
      </div>

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
      description="Headless UI behaviour (focus trap, keyboard nav, ARIA) under daisyUI styling."
    >
      <div className="flex flex-wrap items-center gap-3">
        <Dialog>
          <DialogTrigger as={Button} variant="secondary">
            Open dialog
          </DialogTrigger>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Cancel message?</DialogTitle>
              <DialogDescription>
                This proposes a cancellation to the API — Postgres still decides.
              </DialogDescription>
            </DialogHeader>
            {/* DialogFooter/DialogClose: "unconsumed today" per their own
                doc comments, kept for API parity — mounted here so this
                gallery doesn't silently drop them. */}
            <DialogFooter>
              <DialogClose as={Button} variant="ghost" size="sm">
                Never mind
              </DialogClose>
              <Button variant="destructive" size="sm">
                Cancel message
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        <DropdownMenu>
          <DropdownMenuTrigger as={Button} variant="secondary">
            Row actions
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            <DropdownMenuLabel>cs_msg_001</DropdownMenuLabel>
            <DropdownMenuSeparator />
            {/* DropdownMenuGroup/DropdownMenuCheckboxItem: same "unconsumed
                today, kept for API parity" shape as DialogFooter/DialogClose
                above — mounted for the same reason. */}
            <DropdownMenuGroup>
              <DropdownMenuItem>Copy id</DropdownMenuItem>
              <DropdownMenuItem>Open detail</DropdownMenuItem>
            </DropdownMenuGroup>
            <DropdownMenuSeparator />
            <DropdownMenuCheckboxItem checked>Show masked recipient</DropdownMenuCheckboxItem>
          </DropdownMenuContent>
        </DropdownMenu>

        <Tooltip label="Inferred from prefix — not authoritative.">
          <Button variant="secondary">Hover me</Button>
        </Tooltip>

        <Popover>
          <PopoverTrigger as={Button} variant="secondary">
            Open popover
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
            <Button variant="secondary">Open drawer (generic)</Button>
          </DrawerTrigger>
          <DrawerContent>
            {/* Bug found and fixed by mounting this: with no DrawerTitle,
                vaul's Content (which renders Radix Dialog's Content
                underneath) throws the same dev warning
                primitives/dialog.tsx already guards against. sr-only since
                this demo already shows the id visually in the row below. */}
            <DrawerTitle className="sr-only">cs_msg_001</DrawerTitle>
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
          Fire a success toast
        </Button>
        <Button
          variant="secondary"
          onClick={() =>
            toast({
              title: "Couldn't rotate the secret",
              description: 'missing required permission "webhook:manage"',
              variant: "danger",
            })
          }
        >
          Fire a danger toast
        </Button>
        <Button
          variant="secondary"
          onClick={() => toast({ title: "Replay queued", variant: "default" })}
        >
          Fire a default toast
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
// variants Phase 2's "Delivery" agent builds every Provider/Route/
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
    <Section
      title="Payload inspector"
      description="All three exchange directions — request (outbound to the provider), response would be shown the same way for an adapter that separates them, and callback (an inbound DLR)."
    >
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
            direction: "response",
            method: "POST",
            url: "https://api.orange.cm/smsmessaging/v1/outbound/tel:+237.../requests",
            status: 401,
            durationMs: 88,
            body: '{"requestError":{"serviceException":{"messageId":"SVC0001","text":"Invalid access token"}}}',
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
      description="Diagnose a message without touching SQL. The annotation nodes carry the §4.7 copy for uncertain/undelivered verbatim. Two examples: still in flight (uncertain, annotated) and terminal (delivered, no trailing 'still moving' cap)."
    >
      <div className="flex flex-col gap-4 lg:flex-row">
        <Card className="flex-1">
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
        <Card className="flex-1">
          <CardHeader title="cs_msg_001" meta="+237 6 77 12 34 56 · MTN" />
          <CardBody>
            <StateTimeline
              currentState="delivered"
              isTerminal
              transitions={[
                { toState: "accepted", at: "2026-08-08T14:02:00.000Z", actor: "app:vsms-console" },
                { toState: "queued", at: "2026-08-08T14:02:00.240Z" },
                { toState: "routed", at: "2026-08-08T14:02:00.510Z", providerKey: "orange-cm" },
                {
                  toState: "submitted",
                  at: "2026-08-08T14:02:00.812Z",
                  providerKey: "orange-cm",
                  workerNode: "worker-1",
                  attempt: 1,
                  maxAttempts: 3,
                },
                {
                  toState: "delivered",
                  at: "2026-08-08T14:02:07.100Z",
                  providerKey: "orange-cm",
                },
              ]}
            />
          </CardBody>
        </Card>
      </div>
      <div className="flex flex-col gap-1">
        <p className="text-caption text-muted-foreground">Loading skeleton (no transitions yet):</p>
        <Card>
          <CardBody className="pt-4">
            <StateTimeline currentState="accepted" isTerminal={false} transitions={[]} />
          </CardBody>
        </Card>
      </div>
    </Section>
  );
}

function EncodingPreviewGallery() {
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

function DataDisplayGallery() {
  const longId = "cs_a1b2c3d4e5f6g7h8i9j0k1l2";
  return (
    <Section
      title="Id, MSISDN, and timestamp display"
      description="Design doc §7.1–§7.3: never truncate an MSISDN, never middle-ellipsis an id, never show a bare local time with no zone. Not previously mounted anywhere in this gallery — a real coverage gap, closed in this pass."
    >
      <div className="flex flex-col gap-4">
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            IdDisplay — table variant (7 chars + hover-reveal copy) vs. full (complete, selectable)
          </p>
          <div className="flex flex-wrap items-center gap-6">
            <IdDisplay value={longId} />
            <IdDisplay value={longId} variant="full" />
          </div>
        </div>
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            MsisdnDisplay — with and without a known operator, and an unrecognised shape (falls back
            to the raw string rather than mis-grouping it)
          </p>
          <div className="flex flex-wrap items-center gap-6">
            <MsisdnDisplay value="+237677123456" operator="mtn" />
            <MsisdnDisplay value="+237655123456" operator="orange" />
            <MsisdnDisplay value="+237677123456" />
            <MsisdnDisplay value="not-a-real-msisdn" />
          </div>
        </div>
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            TimestampDisplay — under 24h renders relative (mono, hover for absolute), older falls
            back to the absolute ISO-ordered UTC form. Fixed literal timestamps, not computed from
            `Date.now()` at render time — that computes a different value on the server than on the
            client's own hydration pass (a real hydration-mismatch bug this pass found and fixed:
            React's own "server rendered text didn't match the client" error, reproduced live in the
            console before this fix). `TimestampDisplay`'s own component is fine — it already
            renders the identical absolute string on both passes and only upgrades to relative after
            mounting; the bug was in this gallery's own inline `Date.now()` call, not in `@vsms/ui`.
          </p>
          <div className="flex flex-wrap items-center gap-6">
            <TimestampDisplay value="2026-08-14T15:57:00Z" />
            <TimestampDisplay value="2026-08-14T12:57:00Z" />
            <TimestampDisplay value="2026-01-04T09:12:31Z" />
          </div>
        </div>
        <div>
          <p className="mb-2 text-micro text-subtle-foreground tracking-[0.03em]">
            Overflow check — a long id inside a narrow (200px) container
          </p>
          <div className="w-[200px] rounded-sm border border-edge bg-surface-2 p-2">
            <IdDisplay value={longId} variant="full" />
          </div>
        </div>
      </div>
    </Section>
  );
}

export default function GalleryPage() {
  // D5: DaisyUI's `.tooltip`/`data-tip` needs no provider — no wrapping
  // component here any more (Headless UI has no Tooltip of its own either).
  //
  // Console-redesign Phase 2: this page used to render its own
  // <main max-w-5xl px-6 py-10>, which is now nested inside `ConsoleShell`'s
  // own <main> (Phase 0) — invalid HTML and doubled padding, the identical
  // shape `dashboard-screen.tsx`'s own fix note describes. Replaced with a
  // plain wrapper; the narrower max-w-5xl reading width is kept (a screen
  // may choose its own content width inside the shared shell — the gallery
  // reads better narrower than the 1400px shell default, since it's mostly
  // prose plus small demo blocks, not a dense table). `<Toaster />` is
  // dropped entirely — see this file's own header comment for why.
  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-10">
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
      <JobAndAttemptPillGallery />
      <Separator />
      <StateMarkGallery />
      <Separator />
      <ButtonGallery />
      <Separator />
      <FormGallery />
      <Separator />
      <DataDisplayGallery />
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
    </div>
  );
}
