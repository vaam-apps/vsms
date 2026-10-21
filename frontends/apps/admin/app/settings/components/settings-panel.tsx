// Dumb component (R6, route-local): the Settings screen.
//
// Two sections. Appearance is the one setting this console really owns:
// the theme preference `useTheme` stores in the browser, shown with the
// library's own `ThemeSwitcher` (the same choice the account menu offers
// as a compact select). "Configured elsewhere" says where each of the
// deployment's settings actually lives, because none of them is a
// database row this console can write (see `settings-screen.tsx`).

import { Code, ScreenStack, ThemeSwitcher } from "@vaam-apps/ui";
import { Clock3, KeyRound, Route, SlidersHorizontal } from "lucide-react";
import Link from "next/link";
import type { ReactNode } from "react";
import { PageHeader } from "@/components/page-header";
import { SectionHeader } from "@/components/section-header";

function Elsewhere({
  icon,
  title,
  children,
}: {
  icon: ReactNode;
  title: string;
  children: ReactNode;
}) {
  return (
    <li className="flex gap-4 rounded-[20px] border border-edge-subtle bg-base-100 p-5">
      <span
        aria-hidden="true"
        className="flex size-10 shrink-0 items-center justify-center rounded-full bg-primary-container text-on-primary-container"
      >
        {icon}
      </span>
      <span className="flex min-w-0 flex-col gap-1">
        <span className="text-title-medium text-foreground">{title}</span>
        <span className="text-body-medium text-muted-foreground">{children}</span>
      </span>
    </li>
  );
}

export function SettingsPanel() {
  return (
    <ScreenStack>
      <PageHeader
        title="Settings"
        description="Your appearance in this browser, and where the deployment's own settings live."
      />

      <SectionHeader
        title="Appearance"
        description="Saved in this browser only. System follows your operating system."
      />
      <div className="rounded-[20px] border border-edge-subtle bg-base-100 p-5">
        <ThemeSwitcher aria-label="Theme" />
      </div>

      <SectionHeader
        title="Configured elsewhere"
        description="None of these is a database row, so none is editable from the console."
      />
      <ul className="grid gap-3 lg:grid-cols-2">
        <Elsewhere icon={<Route size={20} />} title="Routing rules">
          Editable, on their own screens:{" "}
          <Link href="/routes" className="text-primary underline underline-offset-2">
            Routes
          </Link>{" "}
          and the{" "}
          <Link href="/simulator" className="text-primary underline underline-offset-2">
            route simulator
          </Link>
          .
        </Elsewhere>
        <Elsewhere icon={<Clock3 size={20} />} title="Marketing quiet hours">
          A policy constant in the gateway (<Code>MARKETING_QUIET_HOURS_START_WAT</Code>,{" "}
          <Code>_END_WAT</Code>), changed by a release rather than a switch.
        </Elsewhere>
        <Elsewhere icon={<KeyRound size={20} />} title="Hashing pepper">
          <Code>SMS_HASH_PEPPER</Code>, set where the gateway is deployed and never readable here.
          Rotating it does not rehash stored rows.
        </Elsewhere>
        <Elsewhere icon={<SlidersHorizontal size={20} />} title="Rate limits and other knobs">
          Flags and environment variables on <Code>sms-gateway serve</Code>, set at deploy time.
        </Elsewhere>
      </ul>
    </ScreenStack>
  );
}
