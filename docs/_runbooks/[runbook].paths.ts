// One page per docs/runbooks/*.adoc, rendered by ../.vitepress/asciidoc.ts.
// This directory only holds the route: config.ts rewrites it to /runbooks/,
// so pages sit at the same path as their sources. The route is kept out of
// docs/runbooks/ itself because `cargo xtask docs-pdf` reads every Markdown
// file in that directory as a chapter.
import { execFileSync } from "node:child_process";
import { basename } from "node:path";
import { renderRunbook } from "../.vitepress/asciidoc";
import { docsDir } from "../.vitepress/links";

function lastCommitMs(file: string): number | undefined {
  try {
    const out = execFileSync("git", ["log", "-1", "--pretty=%at", "--", file], {
      cwd: docsDir,
      encoding: "utf8",
    }).trim();
    return out ? Number(out) * 1000 : undefined;
  } catch {
    return undefined;
  }
}

export default {
  watch: ["../runbooks/*.adoc"],
  paths(files: string[]) {
    return files.sort().map((file) => {
      const name = basename(file, ".adoc");
      const { title, markdown } = renderRunbook(file);
      return {
        params: {
          runbook: name === "README" ? "index" : name,
          source: `runbooks/${name}.adoc`,
          title,
        },
        content: markdown,
      };
    });
  },
  // Edit links and "last updated" belong to the .adoc, not to this route.
  transformPageData(pageData: { params?: Record<string, string> }) {
    const source = pageData.params?.source;
    if (!source) return;
    return { filePath: source, title: pageData.params?.title, lastUpdated: lastCommitMs(source) };
  },
};
