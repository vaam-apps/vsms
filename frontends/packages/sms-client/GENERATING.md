# frontends/packages/sms-client — generated, not committed

This is the one file in `frontends/packages/sms-client/` that is **hand-written and
tracked** — deliberately, so it exists and is readable in a fresh clone
*before* anything has been generated. Everything else here (`src/`,
`dist/`, `tsconfig.json`, `README.md`) is written by
`cratestack generate-typescript` and is **gitignored**, per the owner's
standing rule: auto-generated code is never committed to version control.

`package.json` is the one exception — see the comment above its entry in
the repo root's `.gitignore` for why it has to stay tracked.

## Before you can build anything here

```bash
just client-gen
```

which wraps:

```bash
cratestack generate-typescript --schema schemas/vsms.cstack \
  --out frontends/packages/sms-client --package-name @vsms/sms-client --base-path ''
```

Requires `cratestack` **>=0.7.8** on `PATH` (or `CRATESTACK_BIN=/path/to/cratestack just client-gen`)
— [cratestack#455](https://github.com/cratestack/cratestack/issues/455) / [#456](https://github.com/cratestack/cratestack/pull/456)
fixed `Decimal` scalar TypeScript emission, and vsms uses `Decimal` on
three money fields; anything older regenerates a client that fails to
compile (`Cannot find name 'Decimal'`).

**As of the cratestack 0.7.10 bump, "compiles" changed shape, not just
version.** [cratestack#498](https://github.com/cratestack/cratestack/issues/498)
(landed in [#499](https://github.com/cratestack/cratestack/pull/499)) made
every `Decimal`-typed field a real `decimal.js` `Decimal` *class instance*
instead of a `string` — breaking, per its own CHANGELOG entry. This
package is currently only exercised by `client-check`'s route-matching
gate; nothing in `admin`/`frontends/packages/gateway` imports `@vsms/sms-client` at
runtime (`frontends/packages/gateway/src/client.ts`'s own module doc explains why:
it hand-transcribes the same three `Decimal` fields as plain strings,
deliberately, for the money-safety convention — see that file). So this
break has no live consumer inside vsms today; the moment something does
`import` from this package, the field arrives as a `Decimal` object, not
a `string` or `number`, and has to be handled that way (`.toString()`, not
implicit stringification via template literals, which does happen to work
but is easy to get wrong with arithmetic).

Run this **once, right after `pnpm install`**, before `pnpm run build`,
`pnpm --filter @vsms/sms-client build`, or any `tsc`/`turbo` command that
touches this package. A fresh `git clone` has no `src/` here until you do.

CI (`.github/workflows/ci.yml`'s `js` job) does this automatically, using
the **published** `cratestack` CLI at whatever version is currently pinned
in the root `Cargo.toml` (read via `cargo xtask cratestack-pin`, `=0.7.16`
as of this bump) — never a locally built binary, so the CI gate proves
something on every machine, not just one developer's. This paragraph used
to name a literal version number here as if it were hardcoded in the
workflow; it isn't (see `AGENTS.md`'s xtask section) — worth saying
explicitly since a stale hardcoded-looking version number in prose is
exactly the kind of doc drift this repo keeps finding the hard way.

## Verifying the generated client actually matches the running server

There is deliberately no "does the committed client match the schema"
gate any more — with nothing committed, there is nothing to drift from.
What still matters, and is still checked (`just client-check`, wired into
CI right after generation): **does every route this client calls exist on
the real, pinned `sms-gateway` server?** `ci/assert-client-routes-match-server.mjs`
answers that directly, by building `sms-gateway` from the pinned library
(`=0.6.7`) and diffing its live route table against every call the
generated client makes — the one thing that would silently 404 in
production if the CLI used to generate this client and the server library
it talks to ever drift apart.
