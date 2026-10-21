// Where links in the existing docs point, once those docs are a website.
//
// The Markdown and AsciiDoc under docs/ were written to be read on GitHub,
// so they link to each other by file (`runbooks/sso.adoc`), to the rest of
// the repository by relative path (`../CONTRIBUTING.md`), and to headings by
// GitHub's anchor ids. This module maps each of those onto the site without
// editing a single source file: runbooks become their rendered pages, and
// anything outside docs/ becomes a link to the file on GitHub.
import { existsSync, statSync } from "node:fs";
import { dirname, join, posix } from "node:path";
import { fileURLToPath } from "node:url";

export const repo = "https://github.com/vaam-apps/vsms";
export const branch = "main";

/** The site's base path. `||`, not `??`: PR builds pass an empty string. */
export const base = process.env.DOCS_BASE || "/vsms/";

/** Absolute path of the docs source directory (VitePress's srcDir). */
export const docsDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = join(docsDir, "..");

/** The route a runbook is rendered at, relative to the site root. */
export function runbookRoute(name: string): string {
  return name === "README" ? "runbooks/index.md" : `runbooks/${name}.md`;
}

/**
 * GitHub's heading ids, so `architecture.md#92-deployment` lands on
 * "9.2 Deployment" here exactly as it does on GitHub. VitePress's own
 * slugger would produce `_9-2-deployment` and break every such link.
 */
export function githubSlug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{M}\p{N}\p{Pc}\- ]/gu, "")
    .replace(/ /g, "-");
}

export type Resolved =
  | { kind: "page"; path: string; hash: string }
  | { kind: "external"; url: string };

/**
 * Resolve `href` as written in the source file `from` (relative to docs/).
 * Returns undefined for links that need no change: URLs, in-page anchors,
 * and links to Markdown pages that VitePress already handles.
 */
export function resolveSourceLink(from: string, href: string): Resolved | undefined {
  if (!href || /^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("#") || href.startsWith("/")) {
    return undefined;
  }
  const hashAt = href.indexOf("#");
  const target = decodeURI(hashAt === -1 ? href : href.slice(0, hashAt));
  const hash = hashAt === -1 ? "" : href.slice(hashAt);
  const inRepo = posix.normalize(posix.join("docs", posix.dirname(from), target));

  if (inRepo === "docs/runbooks") {
    return { kind: "page", path: runbookRoute("README"), hash };
  }
  if (inRepo.startsWith("docs/runbooks/") && inRepo.endsWith(".adoc")) {
    const name = posix.basename(inRepo, ".adoc");
    if (!existsSync(join(repoRoot, inRepo))) throw new Error(`${from}: no such runbook ${href}`);
    return { kind: "page", path: runbookRoute(name), hash };
  }
  if (inRepo.startsWith("docs/") && inRepo.endsWith(".md")) {
    return { kind: "page", path: inRepo.slice("docs/".length), hash };
  }

  // Everything else lives outside the site: link to it on GitHub.
  const abs = join(repoRoot, inRepo);
  if (inRepo.startsWith("..") || !existsSync(abs)) {
    throw new Error(`${from}: link to ${href} resolves to ${inRepo}, which does not exist`);
  }
  const kind = statSync(abs).isDirectory() ? "tree" : "blob";
  return { kind: "external", url: `${repo}/${kind}/${branch}/${inRepo}${hash}` };
}

/** A page link as a Markdown-relative href, for VitePress to resolve. */
export function relativePageHref(from: string, page: string, hash: string): string {
  const rel = posix.relative(posix.dirname(from), page);
  return `${rel.startsWith(".") ? rel : `./${rel}`}${hash}`;
}

/** A page link as a final URL, for raw HTML that VitePress does not rewrite. */
export function pageUrl(page: string, hash: string): string {
  const clean = page.replace(/(^|\/)index\.md$/, "$1").replace(/\.md$/, "");
  const slug = hash ? `#${githubSlug(decodeURIComponent(hash.slice(1)))}` : "";
  return `${base}${clean}${slug}`;
}
