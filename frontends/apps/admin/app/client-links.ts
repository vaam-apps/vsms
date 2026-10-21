// Client-side navigation for every plain `<a href>` in the console.
//
// # Why this exists
//
// `@vaam-apps/ui`'s `SideNav` renders its rows as plain `<a href>` (the
// package has no router dependency and no link-component prop), and so
// did several of this console's own links. A plain anchor is a full
// document load: measured on `main` before this file, ten clicks through
// the sidebar were ten document loads, each one re-running middleware,
// re-downloading and re-hydrating the whole app and throwing away every
// cached query, so every screen started from a skeleton. That is the
// "it reloads pages all the time" the owner reported.
//
// Rather than fork the library or wrap each anchor, one capture-phase
// click listener on `document` hands same-origin, unmodified, left-button
// clicks on an internal anchor to the App Router instead. It reaches the
// portalled floating rails and the More sheet too, which a wrapper element
// around `SideNav` could not, because those are mounted on `document.body`.
//
// Everything a browser user expects from a real link is kept: middle-click
// and ⌘/Ctrl/Shift/Alt-click still open a new tab or window, `target`,
// `download` and cross-origin links are left alone, and `/api/*` (the auth
// routes, which must be real requests) is never intercepted.

/** The subset of a `MouseEvent` this decision reads. */
export interface LinkClick {
  button: number;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  defaultPrevented: boolean;
}

/** The subset of an anchor this decision reads. */
export interface LinkTarget {
  href: string;
  target: string;
  hasDownload: boolean;
}

/**
 * The in-app path (pathname + search + hash) a click should navigate to
 * client-side, or `null` when the browser should handle it itself.
 */
export function clientHref(click: LinkClick, link: LinkTarget, origin: string): string | null {
  if (click.defaultPrevented) return null;
  if (click.button !== 0) return null;
  if (click.metaKey || click.ctrlKey || click.shiftKey || click.altKey) return null;
  if (link.hasDownload) return null;
  if (link.target !== "" && link.target !== "_self") return null;
  return internalPath(link.href, origin);
}

/** `href` as an in-app path when it is same-origin and not under `/api/`. */
export function internalPath(href: string, origin: string): string | null {
  let url: URL;
  try {
    url = new URL(href, origin);
  } catch {
    return null;
  }
  if (url.origin !== origin) return null;
  if (url.pathname === "/api" || url.pathname.startsWith("/api/")) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}
