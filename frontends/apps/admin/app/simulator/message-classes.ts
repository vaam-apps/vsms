// The four `MessageClass` values, as data — no JSX, no fetching.
//
// **Duplication, flagged rather than silently fixed**: this exact array
// (`["otp", "transactional", "notification", "marketing"] as const`) also
// appears verbatim in `frontends/apps/admin/app/page.tsx` (the composer) and
// `frontends/apps/admin/app/routes/routes-screen.tsx` (the Routes screen) — three
// independent copies of one decision. This PR only owns the simulator, so
// only this copy is extracted here; the other two are each another agent's
// file and are left untouched, per this PR's own coordination notes. A
// shared `@vsms/ui` (or a small domain package) module is the eventual fix,
// once one PR can safely touch all three call sites.

export const MESSAGE_CLASSES = ["otp", "transactional", "notification", "marketing"] as const;
export type MessageClass = (typeof MESSAGE_CLASSES)[number];
