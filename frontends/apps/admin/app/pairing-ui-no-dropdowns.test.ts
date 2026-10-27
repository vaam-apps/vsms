// The pairing and key screens are new, and the rule that binds them is vaam's:
// no dropdown menus and no dialogs, a choice is shown (buttons) or asked inline
// (`InlineConfirm`). The rest of this console predates the rule and still has
// `Select`s and `Dialog`s, which this file does not touch; it holds the files
// VS-5b wrote to it, so the next edit to one of them cannot reach for a menu.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const HERE = fileURLToPath(new URL(".", import.meta.url));

const FILES = [
  "console-gate.tsx",
  "key-status.tsx",
  "components/account-slot.tsx",
  "components/gate-splash.tsx",
  "components/gate-unreachable.tsx",
  "pair/pair-screen.tsx",
  "pair/components/pair-view.tsx",
  "console/console-key-screen.tsx",
  "console/components/console-key-panel.tsx",
  "../components/qr-code.tsx",
];

const FORBIDDEN = [
  /\bSelect(?:Dropdown|Trigger|Item|Value)?\b/,
  /\bDropdown(?:Menu)?\b/,
  /<select\b/i,
  /<datalist\b/i,
  /\bPopover\b/,
  /\bDialog(?:Content|FullScreen)?\b/,
  /\bConfirmDialog\b/,
  /aria-haspopup/,
  /role=["'](?:menu|listbox|combobox)["']/,
];

describe("the pairing and key screens", () => {
  for (const file of FILES) {
    it(`${file} has no dropdown, popover or dialog`, () => {
      const source = readFileSync(`${HERE}${file}`, "utf8");
      // Comments may name what is banned; code may not use it.
      const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
      for (const pattern of FORBIDDEN) {
        expect(code, `${file} matches ${pattern}`).not.toMatch(pattern);
      }
    });
  }
});
