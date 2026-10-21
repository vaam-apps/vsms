import "@fontsource-variable/roboto-flex/standard.css";
import "@fontsource-variable/jetbrains-mono/index.css";
import { themeInitScript } from "@vaam-apps/ui";
import { env } from "@vsms/env";
import type { Metadata } from "next";
import { headers } from "next/headers";
import "./globals.css";
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
  // `middleware.ts` forwards the signed-in human's email as `x-vsms-actor`;
  // this only displays a value middleware has already validated.
  const headerList = await headers();
  const accountEmail = headerList.get("x-vsms-actor");

  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* biome-ignore lint/security/noDangerouslySetInnerHtml: a fixed literal exported by @vaam-apps/ui */}
        <script dangerouslySetInnerHTML={{ __html: themeInitScript }} />
      </head>
      <body>
        <Providers queryStaleTimeMs={env.QUERY_STALE_TIME_MS}>
          <ConsoleShell accountEmail={accountEmail}>{children}</ConsoleShell>
        </Providers>
      </body>
    </html>
  );
}
