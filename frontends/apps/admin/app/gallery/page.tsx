// The component gallery (T6 / console-redesign.md §7 Phase 2) — the LAST
// screen built in this redesign, deliberately: it imports and exercises
// every `@vsms/ui` export, so it doubles as the console's own visual-QA
// surface (Phase 3's manual pass runs through this page plus one screen
// per IA group). A gallery that silently drops an export is a QA surface
// with a blind spot.
//
// # R6
//
// This file composes only — no `className`, no markup beyond composing
// `./components/*`. Each demo section is its own route-local dumb
// component (R6-reconcile; this page used to be ~1100 lines of markup and
// classes directly, an R6 violation once the rest of the console moved to
// this layering). `Section` (the shared title/description wrapper every
// gallery entry uses) and `GalleryLayout` (the outer max-w-5xl wrapper +
// masthead) live there too.
//
// No `"use client"` here — a plain server shell composing client leaves,
// same convention `routes/page.tsx`/`jobs/page.tsx` already use. Every
// gallery entry that needs interactivity (hooks, event handlers) carries
// its own `"use client"` directive in `./components/*`; `Section` and
// `GalleryLayout` need none, since both only ever receive `children` and
// plain strings.
//
// Two real bugs were found and fixed by the act of mounting everything,
// not by inspection — recorded at their fix site rather than here, so the
// note stays next to the code it describes:
//   1. `<Toaster />` used to be mounted a second time at the bottom of
//      this page, on top of `providers.tsx`'s own app-root mount — every
//      toast rendered twice, stacked at the same position. Removed; this
//      page renders no `<Toaster />` of its own any more.
//   2. `./components/overlays-gallery.tsx`'s plain `<Drawer>` demo
//      originally rendered with no `DrawerTitle` — see that file's own
//      inline comment for the mechanism (vaul's `Content` renders Radix
//      `Dialog.Content` underneath, which requires one).
//
// The nested-Dialog-in-drawer focus-trap investigation (#274) — the
// longest and most valuable piece of prose in this route — lives in
// `./components/nested-dialog-in-drawer-regression.tsx`, next to the demo
// it documents.

import { Separator } from "@vsms/ui";
import { ButtonGallery } from "./components/button-gallery";
import { DataDisplayGallery } from "./components/data-display-gallery";
import { DetailDrawerGallery } from "./components/detail-drawer-gallery";
import { EncodingPreviewGallery } from "./components/encoding-preview-gallery";
import { FormGallery } from "./components/form-gallery";
import { GalleryLayout } from "./components/gallery-layout";
import { JobAndAttemptPillGallery } from "./components/job-and-attempt-pill-gallery";
import { NestedDialogInDrawerRegression } from "./components/nested-dialog-in-drawer-regression";
import { OverlaysGallery } from "./components/overlays-gallery";
import { PayloadInspectorGallery } from "./components/payload-inspector-gallery";
import { StateMarkGallery } from "./components/state-mark-gallery";
import { StateTimelineGallery } from "./components/state-timeline-gallery";
import { StatusPillGallery } from "./components/status-pill-gallery";
import { TableGallery } from "./components/table-gallery";
import { TabsGallery } from "./components/tabs-gallery";

export default function GalleryPage() {
  return (
    <GalleryLayout>
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
      <NestedDialogInDrawerRegression />
      <Separator />
      <PayloadInspectorGallery />
      <Separator />
      <StateTimelineGallery />
      <Separator />
      <EncodingPreviewGallery />
    </GalleryLayout>
  );
}
