<!--
  This template is a convention, not a gate. Nothing in CI enforces it —
  a reviewer reading a PR that skips a section will simply have to ask.
  Delete any section that genuinely does not apply, and say why rather
  than leaving it blank.
-->

## Summary

<!-- What changed, in a sentence or two. -->

## Intent

<!-- Why. Link the real source of truth: an issue (#123), a design doc
     section, or a maintainer decision quoted directly. A link to this
     template, or to a convention in general, is not a source of truth. -->

## Scope

<!-- Which files/areas, and — as importantly — what you deliberately did
     NOT do. A named scope cut is part of the work; an unnamed one is a
     surprise for whoever picks it up next. -->

## Verification

<!-- What you actually ran, and what it printed. Not "tests pass".

     This repository distrusts a green test run on principle, because it
     has been burned by one: `cratestack-sqlx` silently discarded SQLSTATE
     on every write for three releases while the unit tests covering that
     mapping passed, because they built the error by hand. See AGENTS.md's
     #87 section.

     So, where the change adds or relies on a guard: break it, show it
     fail, restore it, show it pass. Paste the failure output. A guard
     never seen to fail is not known to guard anything.

     If the change touches something only observable at runtime — a
     container, a real database, a browser, a provider — say what you ran
     it against. `cargo check` and `pnpm build` have both been green
     through production-breaking bugs in this repo. -->

## Screenshots / Evidence

<!-- Terminal output, a screenshot, a captured request. Optional when the
     Verification section already carries the evidence. -->

## Risk Assessment

<!-- What could break, and what you did about it. "Low risk" on its own
     says nothing — name the thing that would go wrong if you are wrong. -->

## AI Usage Declaration

<!-- Which parts were AI-written, and what a human verified. The point is
     accountability, not disclosure theatre: a human is answerable for
     this change either way. -->

- [ ] A human directed this change and is accountable for it.
- [ ] Claims in this PR were verified against the repository or a running
      system, rather than assumed.

## Reviewer Focus

<!-- Where you actually want eyes. The subtle decision, the thing you are
     least sure about, the alternative you rejected and might be wrong
     about. -->

---

**Checklist**

- [ ] `docs/roadmap.md` checked. (The check is mandatory on every PR; the
      *edit* usually is not — "no edit needed, this changes no milestone,
      gate, dependency or decision" is a complete answer. See AGENTS.md's
      Conventions section for when it does need updating.)
- [ ] Framework or toolchain surprises recorded in `AGENTS.md` §2.0 or the
      relevant section — that table is the most valuable thing here for
      whoever comes next.
- [ ] New R1 exceptions (raw `sqlx`) carry their reasoning in this PR, not
      just an allowlist edit.
