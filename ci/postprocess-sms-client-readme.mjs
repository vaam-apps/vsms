#!/usr/bin/env node
// Prepends a DO-NOT-EDIT banner to packages/sms-client/README.md.
//
// Why this exists rather than a one-off hand-edit: `cratestack generate-typescript
// --check` does a strict, no-exceptions, whole-directory comparison against the
// schema's own output — verified empirically, not assumed. A hand-edited README.md
// (or any extra file anywhere in the package) is reported as drift with no way to
// exempt it (see `just client-check`'s comment in the justfile). So the banner has
// to be produced by something deterministic that both `client-gen` and
// `client-check` run identically, not by editing the generated file directly.
//
// Usage: node ci/postprocess-sms-client-readme.mjs <path-to-README.md>
// Idempotent: running it twice on an already-bannered file is a no-op.

import { readFileSync, writeFileSync } from "node:fs";

const BANNER_START = "<!-- DO-NOT-EDIT-BANNER:START -->";
const BANNER_END = "<!-- DO-NOT-EDIT-BANNER:END -->";

const banner = `${BANNER_START}
> **GENERATED CODE — DO NOT EDIT.** Everything in this package is emitted by
> \`cratestack generate-typescript\` from \`schema/schema.cstack\`. Hand edits are
> silently discarded (and diverge from CI's drift gate) the next time someone
> regenerates. To change this package, change the schema instead, then run:
>
> \`\`\`
> just client-gen
> \`\`\`
>
> which wraps:
>
> \`\`\`
> cratestack generate-typescript --schema schema/schema.cstack \\
>   --out packages/sms-client --package-name @vsms/sms-client --base-path ''
> \`\`\`
>
> CI enforces this with \`just client-check\`, which runs two independent gates:
> Gate A regenerates into a scratch directory (through this same banner step) and
> diffs it byte-for-byte against this directory — any drift from the schema fails
> it. Gate B (\`ci/assert-client-routes-match-server.mjs\`) checks every route this
> client calls against the real, pinned server's route table — see that script's
> header for why it exists independently of Gate A.
${BANNER_END}

`;

const path = process.argv[2];
if (!path) {
  console.error("usage: postprocess-sms-client-readme.mjs <path-to-README.md>");
  process.exit(1);
}

const original = readFileSync(path, "utf8");
if (original.includes(BANNER_START)) {
  // Already bannered (e.g. re-run) — leave as is.
  process.exit(0);
}

writeFileSync(path, banner + original);
