// Renders docs/runbooks/*.adoc as site pages, leaving the AsciiDoc where it
// is: the runbooks stay the source of truth, read on GitHub and by
// `cargo xtask docs-pdf` exactly as before.
//
// Asciidoctor converts each runbook to HTML, with a few changes so the result
// reads like the rest of the site: code blocks come back out as Markdown
// fences (VitePress highlights them and adds the copy button), admonitions
// become the theme's custom blocks (it styles note, tip, important, warning
// and caution: AsciiDoc's five admonition names), headings get permalinks,
// and links are resolved the same way the Markdown pages' links are (see
// ./links.ts).
//
// The HTML then goes through parse5 and back. Vue compiles every page as a
// template and, unlike a browser, refuses mis-nested tags, which Asciidoctor
// does emit for some inputs (an unescaped `v*.*.*` inside bold text, for one).
import { readFileSync } from "node:fs";
import { basename } from "node:path";
import Asciidoctor from "@asciidoctor/core";
import { parseFragment, serialize, defaultTreeAdapter as tree } from "parse5";
import { pageUrl, resolveSourceLink } from "./links";

const asciidoctor = Asciidoctor();
const html5 = asciidoctor.Html5Converter.create();

// Stands in for a code block until the HTML is final. Plain text, so parse5
// keeps it as it is.
const FENCE = "@@vsms-fence-";

// biome-ignore lint/suspicious/noExplicitAny: Asciidoctor.js's node types are not exported per kind.
type Node = any;
type Element = ReturnType<typeof tree.createElement>;

class SiteConverter {
  fences: string[] = [];

  convert(node: Node, transform?: string, opts?: unknown): string {
    const name = transform ?? node.getNodeName();
    if (name === "listing" || name === "literal") {
      const lang = name === "listing" ? (node.getAttribute("language") ?? "text") : "text";
      const source: string = node.getSource();
      const longest = Math.max(0, ...[...source.matchAll(/`+/g)].map((m) => m[0].length));
      const ticks = "`".repeat(Math.max(3, longest + 1));
      const title = node.getTitle() ? `<div class="title">${node.getTitle()}</div>\n` : "";
      this.fences.push(`${ticks}${lang}\n${source}\n${ticks}`);
      return `${title}${FENCE}${this.fences.length - 1}@@`;
    }
    if (name === "admonition") {
      const kind = node.getAttribute("name");
      const title = node.getTitle() ?? node.getAttribute("textlabel") ?? node.getCaption();
      return `<div class="${kind} custom-block"><p class="custom-block-title">${title}</p>\n${node.getContent()}\n</div>`;
    }
    return html5.convert(node, transform, opts);
  }
}

function textOf(node: Element): string {
  return node.childNodes
    .map((child) =>
      tree.isTextNode(child) ? child.value : "childNodes" in child ? textOf(child as Element) : "",
    )
    .join("");
}

/** Resolve links and add heading permalinks, in place. */
function rewrite(node: Element, from: string): void {
  for (const child of node.childNodes) {
    if (!tree.isElementNode(child)) continue;
    const href = child.attrs.find((a) => a.name === "href");
    if (child.tagName === "a" && href) {
      const resolved = resolveSourceLink(from, href.value);
      if (resolved?.kind === "page") href.value = pageUrl(resolved.path, resolved.hash);
      if (resolved?.kind === "external") {
        href.value = resolved.url;
        child.attrs.push({ name: "target", value: "_blank" }, { name: "rel", value: "noreferrer" });
      }
    }
    const id = child.attrs.find((a) => a.name === "id")?.value;
    if (/^h[2-6]$/.test(child.tagName) && id) {
      // The same permalink VitePress gives a Markdown heading.
      child.attrs.push({ name: "tabindex", value: "-1" });
      const anchor = tree.createElement("a", child.namespaceURI, [
        { name: "class", value: "header-anchor" },
        { name: "href", value: `#${id}` },
        { name: "aria-label", value: `Permalink to "${textOf(child)}"` },
      ]);
      tree.insertText(anchor, "\u200b");
      tree.insertText(child, " ");
      tree.appendChild(child, anchor);
    }
    rewrite(child, from);
  }
}

export interface RenderedRunbook {
  title: string;
  markdown: string;
}

/** Render one runbook to the Markdown a VitePress dynamic route serves. */
export function renderRunbook(file: string): RenderedRunbook {
  const name = basename(file);
  const from = `runbooks/${name}`;
  const converter = new SiteConverter();
  asciidoctor.ConverterFactory.register(converter, ["html5"]);
  const doc = asciidoctor.load(readFileSync(file, "utf8"), {
    safe: "safe",
    // xrefs keep their .adoc target, so links.ts can resolve them.
    attributes: { outfilesuffix: ".adoc" },
  });
  const title = String(doc.getDocumentTitle({ sanitize: true }) ?? name);

  const fragment = parseFragment(doc.convert());
  rewrite(fragment as unknown as Element, from);
  const html = serialize(fragment)
    // Vue would read `{{` as an interpolation: keep it literal.
    .replace(/\{\{/g, "&#123;&#123;");

  // Re-insert the code blocks as fences. A blank line on each side ends the
  // surrounding raw-HTML block, so markdown-it parses the fence as a fence.
  const markdown = html.replace(
    new RegExp(`${FENCE}(\\d+)@@`, "g"),
    (_m, i) => `\n\n${converter.fences[Number(i)]}\n\n`,
  );

  return { title, markdown: `# ${title}\n\n${markdown}\n` };
}
