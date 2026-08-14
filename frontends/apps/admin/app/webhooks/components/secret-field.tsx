"use client";

import { Button } from "@vsms/ui";
import { useState } from "react";
import { maskSecret } from "../webhook-domain";

// Dumb (R6): a masked-by-default secret value with Reveal/Copy. `revealed`/
// `copied` are genuinely ephemeral, single-purpose presentational state
// local to this one field — R6's own carve-out for `useState` inside a
// dumb component.
export function SecretField({ label, value }: { label: string; value: string }) {
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);

  async function copy() {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <div className="flex flex-col gap-1">
      <p className="text-caption text-subtle-foreground">{label}</p>
      <div className="flex items-center gap-2">
        <code className="flex-1 truncate rounded-sm border border-edge bg-surface-2 px-2 py-1 font-mono text-caption text-foreground">
          {revealed ? value : maskSecret(value)}
        </code>
        <Button type="button" variant="ghost" size="sm" onClick={() => setRevealed(!revealed)}>
          {revealed ? "Hide" : "Reveal"}
        </Button>
        <Button type="button" variant="ghost" size="sm" onClick={copy}>
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
}
