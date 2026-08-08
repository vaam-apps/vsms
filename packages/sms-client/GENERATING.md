# packages/sms-client — generated, not committed

This is the one file in `packages/sms-client/` that is **hand-written and
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
cratestack generate-typescript --schema schema/schema.cstack \
  --out packages/sms-client --package-name @vsms/sms-client --base-path ''
```

Requires `cratestack` **>=0.7.8** on `PATH` (or `CRATESTACK_BIN=/path/to/cratestack just client-gen`)
— [cratestack#455](https://github.com/cratestack/cratestack/issues/455) / [#456](https://github.com/cratestack/cratestack/pull/456)
fixed `Decimal` scalar TypeScript emission, and vsms uses `Decimal` on
three money fields; anything older regenerates a client that fails to
compile (`Cannot find name 'Decimal'`).

Run this **once, right after `pnpm install`**, before `pnpm run build`,
`pnpm --filter @vsms/sms-client build`, or any `tsc`/`turbo` command that
touches this package. A fresh `git clone` has no `src/` here until you do.

CI (`.github/workflows/ci.yml`'s `js` job) does this automatically, using
the **published** `cratestack` 0.7.8 — never a locally built binary, so
the CI gate proves something on every machine, not just one developer's.

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
