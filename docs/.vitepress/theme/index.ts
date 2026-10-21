// The vaam-apps house theme (Material 3, configured in ../config.ts), plus a
// component that draws the Mermaid diagrams the design docs carry, and a
// little CSS for the runbooks rendered from AsciiDoc.
import Md3Theme from "@vaam-apps/vitepress-md3";
import type { Theme } from "vitepress";
import Mermaid from "./Mermaid.vue";
import "./asciidoc.css";

export default {
  extends: Md3Theme,
  enhanceApp({ app }) {
    app.component("Mermaid", Mermaid);
  },
} satisfies Theme;
