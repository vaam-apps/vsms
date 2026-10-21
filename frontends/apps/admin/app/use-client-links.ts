"use client";

// The hook half of `client-links.ts`: one document-level click listener
// that hands internal anchors to the App Router, and one that prefetches
// their route when the pointer or keyboard focus reaches them, so the
// navigation that follows is usually already in the router cache.
//
// Bubble phase, not capture: React 19 delegates its own handlers at the root
// container, so a component that handles a link click itself (the More
// sheet closing on a destination, for one) runs first, and a handler that
// calls `preventDefault` opts its link out of this one.

import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { clientHref, internalPath } from "./client-links";

function anchorOf(target: EventTarget | null): HTMLAnchorElement | null {
  if (!(target instanceof Element)) return null;
  const anchor = target.closest("a[href]");
  return anchor instanceof HTMLAnchorElement ? anchor : null;
}

export function useClientLinks(): void {
  const router = useRouter();

  useEffect(() => {
    const origin = window.location.origin;
    const prefetched = new Set<string>();

    function onClick(event: MouseEvent) {
      const anchor = anchorOf(event.target);
      if (anchor === null) return;
      const href = clientHref(
        event,
        { href: anchor.href, target: anchor.target, hasDownload: anchor.hasAttribute("download") },
        origin,
      );
      if (href === null) return;
      event.preventDefault();
      router.push(href);
    }

    function onIntent(event: Event) {
      const anchor = anchorOf(event.target);
      if (anchor === null) return;
      const path = internalPath(anchor.href, origin);
      if (path === null || prefetched.has(path)) return;
      prefetched.add(path);
      router.prefetch(path);
    }

    document.addEventListener("click", onClick);
    document.addEventListener("pointerover", onIntent, { passive: true });
    document.addEventListener("focusin", onIntent);
    return () => {
      document.removeEventListener("click", onClick);
      document.removeEventListener("pointerover", onIntent);
      document.removeEventListener("focusin", onIntent);
    };
  }, [router]);
}
