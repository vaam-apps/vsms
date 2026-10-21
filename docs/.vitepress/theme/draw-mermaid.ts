// Draws one Mermaid diagram in the site's Material 3 colours. Mermaid keeps
// global state (initialize applies to every diagram after it), so diagrams
// draw one at a time, page-wide, even when several mount at once.
let queue: Promise<unknown> = Promise.resolve();
let renders = 0;

function token(name: string): string {
  return getComputedStyle(document.documentElement)
    .getPropertyValue(`--md-sys-color-${name}`)
    .trim();
}

async function drawNow(source: string, dark: boolean): Promise<string> {
  const { default: mermaid } = await import("mermaid");
  // Mermaid sizes each box from measured text: measure with the real font.
  await document.fonts.ready;
  mermaid.initialize({
    startOnLoad: false,
    theme: "base",
    darkMode: dark,
    fontFamily: getComputedStyle(document.body).fontFamily,
    themeVariables: {
      background: token("surface"),
      primaryColor: token("primary-container"),
      primaryTextColor: token("on-primary-container"),
      primaryBorderColor: token("primary"),
      secondaryColor: token("secondary-container"),
      tertiaryColor: token("surface-container"),
      clusterBkg: token("surface-container"),
      clusterBorder: token("outline-variant"),
      edgeLabelBackground: token("surface"),
      lineColor: token("outline"),
      textColor: token("on-surface"),
      labelTextColor: token("on-surface"),
      secondaryTextColor: token("on-surface"),
      tertiaryTextColor: token("on-surface"),
      titleColor: token("on-surface"),
      noteBkgColor: token("secondary-container"),
      noteTextColor: token("on-secondary-container"),
    },
  });
  renders += 1;
  const id = `mermaid-${renders}`;
  return (await mermaid.render(id, source)).svg;
}

/** The diagram as SVG markup; rejects if Mermaid cannot parse it. */
export function drawMermaid(source: string, dark: boolean): Promise<string> {
  const next = queue.then(
    () => drawNow(source, dark),
    () => drawNow(source, dark),
  );
  queue = next.catch(() => undefined);
  return next;
}
