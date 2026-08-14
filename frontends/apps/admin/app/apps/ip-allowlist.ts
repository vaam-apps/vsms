// Pure helpers for the App edit/create forms' IP-allowlist textarea, which
// this screen edits as newline-separated text but the wire format is a
// `string[]` (R6: extracted pure modules carry tests — see
// `ip-allowlist.test.ts`).

export function toIpAllowlistLines(entries: string[]): string {
  return entries.join("\n");
}

export function parseIpAllowlistLines(text: string): string[] {
  return text
    .split(/\r?\n|,/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}
