"use client";

import { useEffect, useState } from "react";
import { Button } from "./button";

/**
 * Flips `<html data-theme="...">` between `dark`/`light` (`admin/app/
 * layout.tsx` sets `dark` as the default, per design doc §3.1.1). Reads
 * the current attribute on mount rather than assuming `dark`, so it stays
 * correct if a future persisted-preference mechanism sets it before this
 * component ever renders.
 *
 * Originally written inline in the composer screen; factored out here
 * once the messages list screen needed the identical control rather than
 * a second copy.
 */
export function ThemeToggle() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");

  useEffect(() => {
    const current = document.documentElement.getAttribute("data-theme");
    if (current === "light" || current === "dark") setTheme(current);
  }, []);

  function flip() {
    const next = theme === "dark" ? "light" : "dark";
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  }

  return (
    <Button variant="secondary" size="sm" onClick={flip} type="button">
      {theme === "dark" ? "Switch to light" : "Switch to dark"}
    </Button>
  );
}
