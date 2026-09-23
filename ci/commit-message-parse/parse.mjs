// The message a squash-merge will land on the default branch must parse with
// the same parser release-please uses.
//
// # Why this exists
//
// release-please does not merely ignore a commit it cannot parse — it drops
// the commit entirely, from the changelog AND from the version calculation,
// and says so only at debug level inside a workflow that reports success:
//
//     ❯ commit could not be parsed: 7ef89ec fix(ci): v0.3.2 published nothing …
//     ❯ error message: Error: unexpected token '(' at 8:30, valid tokens [)]
//     ❯ commits: 0
//     ✔ No commits for path: ., skipping
//
// A real incident, not a hypothetical. That was the ONLY commit since the
// previous release, so release-please proposed no release at all. Nothing was
// red; the `release-please` workflow reported success. The symptom is a
// release PR that never appears — at a glance indistinguishable from "nobody
// has merged anything worth releasing yet."
//
// # The rule the grammar actually enforces
//
// `@conventional-commits/parser` is a strict PEG parser, not the lenient
// regex-based `conventional-commits-parser`. A BODY line that *begins* with
// `identifier(` reads to the grammar as a type-and-scope header, so a nested
// `(` inside it is a syntax error. Measured against the parser, not assumed:
//
//     see A(B(c)) here     OK    — a word precedes it, so it is not a header
//     A(b) here            OK    — single parens, nothing nests
//     A(B(c)) here         FAIL  — line-initial and nested
//     `A(B(c))` here       FAIL  — a backtick does not help
//
// # Why this checks the squash result rather than each commit
//
// Individual commits on a branch do NOT need to be conventional: this
// repository squash-merges with `PR_TITLE`, so only the assembled message
// reaches the default branch, and `wip:` commits on a branch are legitimate.
// A first cut of this guard checked each commit and rejected 24 of the last
// 40 on `main`, almost all for pre-convention subjects that never landed as
// their own commit — a guard that loud gets deleted rather than obeyed.
//
// # How faithful the reconstruction is, stated exactly
//
// Checked against the real #402 merge commit: the title line, the blank line,
// and every `* <subject>` / blank / `<body>` / blank group reproduce it
// byte-for-byte through line 80 of 84. The remaining four lines are GitHub's
// own co-author aggregation footer (a `---------` separator and deduplicated
// `Co-authored-by:` trailers), which this script does not attempt to
// reproduce — that would mean reimplementing GitHub's dedup. It cannot change
// the verdict: a `token: value` trailer is a well-formed footer, and both the
// passing and failing cases above were re-checked with and without it and
// gave identical results.

import { execFileSync } from "node:child_process";
import { parser } from "@conventional-commits/parser";

const { PR_NUMBER, PR_TITLE, GH_REPO } = process.env;
for (const [name, value] of Object.entries({ PR_NUMBER, PR_TITLE, GH_REPO })) {
  if (!value) {
    console.error(`${name} is not set`);
    process.exit(2);
  }
}

const messages = JSON.parse(
  execFileSync(
    "gh",
    [
      "api",
      `repos/${GH_REPO}/pulls/${PR_NUMBER}/commits`,
      "--paginate",
      "--jq",
      "[.[].commit.message]",
    ],
    { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  ),
);

const parts = [`${PR_TITLE} (#${PR_NUMBER})`, ""];
for (const message of messages) {
  const [subject, ...rest] = message.split("\n");
  parts.push(`* ${subject}`, "");
  const body = rest.join("\n").replace(/^\n+|\n+$/g, "");
  if (body) parts.push(body, "");
}
const squash = parts.join("\n").replace(/\n+$/, "");

try {
  parser(squash);
  console.log(
    `OK — the squash message for #${PR_NUMBER} (${messages.length} commit(s), ` +
      `${squash.split("\n").length} lines) parses as a conventional commit`,
  );
} catch (error) {
  const detail = String(error?.message ?? error);
  const lines = squash.split("\n");
  console.error(`\n✗ the squash message for #${PR_NUMBER} cannot be parsed`);
  console.error(`  ${detail}`);

  const at = /at (\d+):(\d+)/.exec(detail);
  const offending = at ? lines[Number(at[1]) - 1] : undefined;
  if (at && offending !== undefined) {
    const gutter = `  line ${at[1]}: `;
    console.error(`\n${gutter}${offending}`);
    console.error(`${" ".repeat(gutter.length + Number(at[2]) - 1)}^`);
  }

  console.error(`
release-please would DROP this commit entirely — no changelog entry, and no
contribution to the version bump. If it is the only commit since the last
release, no release PR is proposed at all and nothing reports an error.

The usual cause is a body line that BEGINS with a call-like token containing
nested parentheses, which the grammar reads as a type-and-scope header:

    A(B(c)) is a build failure       <- line-initial and nested: rejected
    \`A(B(c))\` is a build failure     <- a backtick does not help
    see A(B(c)) here                 <- fine, a word precedes it
    A(b) here                        <- fine, nothing nests

Reword the offending line in the commit it came from, then force-push.
Editing the pull request title alone will not fix a body line.`);
  process.exit(1);
}
