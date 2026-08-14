import type { Metadata } from "next";
import { headers } from "next/headers";
import "./globals.css";
import { ConsoleShell } from "./console-shell";
import { Providers } from "./providers";

export const metadata: Metadata = {
  title: "vsms Admin Console",
  description: "A2P SMS gateway admin dashboard",
};

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  // `frontends/apps/admin/middleware.ts` forwards the signed-in human's email as
  // `x-vsms-actor` on every authenticated request — the same header
  // `frontends/packages/api/src/context.ts` already reads for the tRPC `actor`
  // field. Reading it here, rather than importing `frontends/apps/admin/lib/session.ts`
  // directly, keeps this file free of auth *logic*: it only displays a
  // value middleware has already validated and set (or left absent, on
  // `/login`, where `ConsoleShell` renders no chrome at all).
  const headerList = await headers();
  const accountEmail = headerList.get("x-vsms-actor");

  return (
    <html lang="en" data-theme="dark">
      <body>
        <Providers>
          <ConsoleShell accountEmail={accountEmail}>{children}</ConsoleShell>
        </Providers>
      </body>
    </html>
  );
}
