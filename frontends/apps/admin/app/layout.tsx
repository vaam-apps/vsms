import "@fontsource-variable/roboto-flex/standard.css";
import "@fontsource-variable/jetbrains-mono/index.css";
import { themeInitScript } from "@vaam-apps/ui";
import { env } from "@vsms/env";
import type { Metadata } from "next";
import { connection } from "next/server";
import "./globals.css";
import type { ConsoleConfig } from "@/lib/console-config";
import { ConsoleGate } from "./console-gate";
import { ConsoleShell } from "./console-shell";
import { Providers } from "./providers";

export const metadata: Metadata = {
  title: { default: "vsms console", template: "%s · vsms console" },
  description: "A2P SMS gateway admin console",
};

// Fonts are self-hosted from npm (`@fontsource-variable/*`, OFL-1.1) rather
// than linked from Google Fonts: the console must render the same in a
// deployment with no route to the public internet, and Next serves the woff2
// files from its own origin. `theme.css` already names "Roboto Flex
// Variable" and "JetBrains Mono Variable" in its stacks. `standard.css`
// carries the weight, width, slant and optical-size axes; the slant axis is
// what `--font-italic` draws with.
//
// `themeInitScript` sets `data-theme` from the stored preference before the
// first paint, so a visitor who chose "light" never sees a dark flash; it is
// why `<html>` carries `suppressHydrationWarning` (the script changes an
// attribute React did not render). It is a fixed literal from the library.

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  // The gateway's two PUBLIC sealing keys are runtime configuration, read here
  // per request and handed to the browser, which pins them. `connection()` opts
  // the whole tree out of static rendering: a build has no keys, and a value
  // baked in at build time would pin one environment's keys into an image that
  // is promoted to another.
  await connection();
  const config: ConsoleConfig = {
    serverKeys: {
      current: env.SMS_ENVELOPE_SERVER_KEY_CURRENT,
      next: env.SMS_ENVELOPE_SERVER_KEY_NEXT,
    },
    messageStreamPollMs: env.MESSAGE_STREAM_POLL_MS,
  };

  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* biome-ignore lint/security/noDangerouslySetInnerHtml: a fixed literal exported by @vaam-apps/ui */}
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
      <body>
        <Providers config={config} queryStaleTimeMs={env.QUERY_STALE_TIME_MS}>
          <ConsoleGate>
            <ConsoleShell>{children}</ConsoleShell>
          </ConsoleGate>
        </Providers>
      </body>
    </html>
  );
}
