// The pairing and key screens, and the screens written after them, are new, and
// the rule that binds them is vaam's: no dropdown menus and no dialogs, a choice
// is shown (buttons, a radio group, chips) or asked inline (`InlineConfirm`).
// The rest of this console predates the rule and still has `Select`s and
// `Dialog`s, which this file does not touch; it holds the files written to the
// rule, so the next edit to one of them cannot reach for a menu. A new screen
// that follows the rule adds its files here. The scan reads copy as well as
// code, so a label says "Choose", never the banned control's own name.

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
  "delivery-lookup/page.tsx",
  "delivery-lookup/delivery-lookup-screen.tsx",
  "delivery-lookup/lookup-query.ts",
  "delivery-lookup/lookup-error.ts",
  "delivery-lookup/lookup-labels.ts",
  "delivery-lookup/components/lookup-form.tsx",
  "delivery-lookup/components/lookup-results.tsx",
  "delivery-lookup/components/lookup-detail-fields.tsx",
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

describe("the pairing and key screens, and the screens written after them", () => {
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
