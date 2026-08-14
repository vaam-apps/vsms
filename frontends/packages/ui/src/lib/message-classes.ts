// The `MessageClass` vocabulary (`schema.cstack`'s `MessageClass` enum) —
// shared across every screen that needs to render or filter by it.
//
// Extracted from routes-screen.tsx's own copy of a triplicated
// `const MESSAGE_CLASSES = [...] as const` (the other two live in
// `frontends/apps/admin/app/page.tsx`'s composer and
// `frontends/apps/admin/app/simulator/simulator-screen.tsx`, both owned by
// other in-flight work at the time of this extraction — R6's own
// "route-local vs shared" test says this belongs here the moment a second
// screen would plausibly use it, which is already true of all three). Only
// this file's own two call sites (routes-screen.tsx) were updated to import
// from here; the other two still carry their own local `const` as of this
// change, flagged rather than silently fixed out from under a parallel PR.
export const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;
export type MessageClass = (typeof MESSAGE_CLASSES)[number];
