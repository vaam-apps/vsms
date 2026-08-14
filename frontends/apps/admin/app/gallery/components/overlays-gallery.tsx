"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import {
  Button,
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
  Popover,
  PopoverContent,
  PopoverTrigger,
  Tooltip,
  toast,
} from "@vsms/ui";
import { Section } from "./section";

export function OverlaysGallery() {
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
