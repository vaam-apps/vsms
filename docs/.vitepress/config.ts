import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";
import { base, githubSlug, relativePageHref, repo, resolveSourceLink } from "./links";

// Served from GitHub Pages at /vsms/. The docs workflow passes the path Pages
// reports in DOCS_BASE, so a custom domain later needs no change here.
export default defineMd3Config({
  base,
  lang: "en",
  title: "vsms",
  titleTemplate: ":title · vsms",
  description: "A Cameroon-first, extensible A2P SMS gateway for OTPs and notifications.",
  cleanUrls: true,
  lastUpdated: true,
  // A link to a page that does not exist fails the build.
  ignoreDeadLinks: false,
  head: [["link", { rel: "icon", type: "image/svg+xml", href: `${base}logo.svg` }]],

  // The runbook route lives in _runbooks/ (see the comment in its paths file)
  // and is served at /runbooks/, next to where its sources live.
  rewrites: { "_runbooks/:runbook": "runbooks/:runbook" },

  markdown: {
    // GitHub's heading ids: the docs link to each other by them.
    anchor: { slugify: githubSlug },
    // CrateStack's schema language is Prisma-shaped (datasource, enum, model,
    // @@allow), close enough to highlight with Prisma's grammar.
    languageAlias: { cstack: "prisma" },
    config(md) {
      // Links written for GitHub: runbooks (.adoc) go to their rendered page,
      // anything outside docs/ goes to the file on GitHub. See ./links.ts.
      const linkOpen = md.renderer.rules.link_open;
      md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
        const token = tokens[idx];
        const href = token?.attrGet("href");
        const resolved = href ? resolveSourceLink(env.relativePath, href) : undefined;
        if (token && resolved?.kind === "external") token.attrSet("href", resolved.url);
        if (token && resolved?.kind === "page" && !href?.endsWith(".md")) {
          token.attrSet("href", relativePageHref(env.relativePath, resolved.path, resolved.hash));
        }
        return linkOpen
          ? linkOpen(tokens, idx, options, env, self)
          : self.renderToken(tokens, idx, options);
      };

      // ```mermaid blocks are drawn by the Mermaid component (theme/), with
      // the highlighted source as its fallback.
      const fence = md.renderer.rules.fence;
      md.renderer.rules.fence = (tokens, idx, options, env, self) => {
        const html = fence ? fence(tokens, idx, options, env, self) : "";
        const token = tokens[idx];
        if (token?.info.trim() !== "mermaid") return html;
        return `<Mermaid code="${encodeURIComponent(token.content)}">${html}</Mermaid>`;
      };
    },
  },

  themeConfig: {
    md3: {
      style: "expressive",
      seed: "#6d28d9",
    },

    logo: "/logo.svg",

    nav: [
      { text: "Get started", link: "/what-is-vsms", activeMatch: "^/what-is-vsms" },
      { text: "Integrate", link: "/integrating", activeMatch: "^/(integrating|sdks)" },
      { text: "Architecture", link: "/architecture", activeMatch: "^/(architecture|design/)" },
      { text: "Runbooks", link: "/runbooks/", activeMatch: "^/runbooks/" },
      {
        text: "More",
        items: [
          { text: "Roadmap", link: "/roadmap" },
          { text: "Data retention briefing", link: "/legal/retention-briefing" },
          { text: "Contributing", link: `${repo}/blob/main/CONTRIBUTING.md` },
          { text: "Changelog", link: `${repo}/blob/main/CHANGELOG.md` },
          {
            text: "All vaam-apps docs",
            link: "https://vaam-apps.github.io/vitepress-md3/projects/",
          },
        ],
      },
    ],

    sidebar: [
      {
        text: "Getting started",
        items: [
          { text: "What is vsms?", link: "/what-is-vsms" },
          { text: "Local development", link: "/runbooks/local-development" },
          { text: "Showcase (published images)", link: "/runbooks/showcase" },
          { text: "First-time setup, the long way", link: "/runbooks/getting-started" },
        ],
      },
      {
        text: "Integrating",
        items: [
          { text: "Integrating with vsms", link: "/integrating" },
          { text: "SDKs", link: "/sdks" },
        ],
      },
      {
        text: "Architecture and design",
        items: [
          { text: "Architecture", link: "/architecture" },
          { text: "Multi-country", link: "/design/multi-country" },
          { text: "Console redesign", link: "/design/console-redesign" },
          { text: "Console pairing", link: "/design/console-pairing" },
          { text: "Frontend package audit", link: "/design/frontend-package-audit" },
          { text: "better-auth evaluation", link: "/design/better-auth-evaluation" },
          { text: "ADR: GDPR readiness", link: "/design/gdpr-engineering-readiness-adr" },
          { text: "RFC: GDPR readiness", link: "/design/gdpr-engineering-readiness-rfc" },
        ],
      },
      {
        text: "Operating",
        items: [
          { text: "Runbooks", link: "/runbooks/" },
          { text: "Deployment", link: "/runbooks/deployment" },
          { text: "Backup and restore", link: "/runbooks/backup-restore" },
          { text: "Alerting", link: "/runbooks/alerting" },
          { text: "Running the CI gate locally", link: "/runbooks/testing" },
          { text: "Releasing the SDKs", link: "/runbooks/releasing-sdks" },
          { text: "#160: integration story", link: "/runbooks/e2e-integration" },
          { text: "#64: grey-route validation", link: "/runbooks/grey-route-validation" },
          { text: "#36: real handset gate", link: "/runbooks/36-handset-gate" },
          { text: "#65: kill Orange gate", link: "/runbooks/65-kill-orange-gate" },
        ],
      },
      {
        text: "Roadmap",
        items: [
          { text: "Roadmap", link: "/roadmap" },
          { text: "Open questions", link: `${repo}/blob/main/OPEN_QUESTIONS.md` },
        ],
      },
      {
        text: "Legal",
        items: [{ text: "Data retention briefing", link: "/legal/retention-briefing" }],
      },
    ],

    outline: { level: [2, 3] },
    search: { provider: "local" },
    socialLinks: [{ icon: "github", link: repo }],
    editLink: {
      pattern: `${repo}/edit/main/docs/:path`,
      text: "Edit this page on GitHub",
    },
    footer: {
      message: "Released under the MIT License.",
    },
  },
});
