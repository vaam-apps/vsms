// Dumb view for the Settings screen (R6) — markup and classes only, no data
// fetching, no business rules. Content is static by design; see
// `settings-screen.tsx`'s own module doc for why there is nothing to fetch
// or edit here.

import { Code, InlineBanner, ScreenHeader, ScreenStack } from "@vsms/ui";

export function SettingsPanel() {
  return (
    <ScreenStack>
      <ScreenHeader
        title="Settings"
        description="There is no runtime-configurable settings model in this system today."
      />

      <InlineBanner variant="neutral">
        This screen exists because #58 named &quot;settings&quot; as one of the remaining pages, not
        because there is a <Code>Settings</Code> model to build a form around — there isn&apos;t
        one. Rather than invent switches that don&apos;t connect to anything, here is where the
        things a &quot;settings&quot; screen might otherwise show actually live today.
      </InlineBanner>

      <div className="flex flex-col divide-y divide-edge rounded-sm border border-edge">
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">Marketing quiet hours</p>
          <p className="text-caption text-muted-foreground">
            A self-imposed policy constant (§10), not a database row —{" "}
            <Code>backends/crates/sms-api/src/consent.rs::MARKETING_QUIET_HOURS_START_WAT</Code> /{" "}
            <Code>_END_WAT</Code>. Deliberately not runtime-editable: the source comment is explicit
            that "the next reader should see a value to reconsider, not a rule to trust."
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">MSISDN/body hashing pepper</p>
          <p className="text-caption text-muted-foreground">
            <Code>SMS_HASH_PEPPER</Code>, a process environment variable the gateway validates at
            startup — never read or writable from this console. Rotating it does not rehash stored
            rows; see <Code>OPEN_QUESTIONS.md</Code> §3.1.
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">Provider routing rules</p>
          <p className="text-caption text-muted-foreground">
            Configurable, and already has a real screen — see{" "}
            <a href="/routes" className="underline decoration-edge-strong underline-offset-2">
              Routes
            </a>{" "}
            and the{" "}
            <a href="/simulator" className="underline decoration-edge-strong underline-offset-2">
              route simulator
            </a>
            .
          </p>
        </div>
        <div className="flex flex-col gap-1 px-4 py-3">
          <p className="font-medium text-body text-foreground">
            Rate limits, idempotency TTLs, and other operational knobs
          </p>
          <p className="text-caption text-muted-foreground">
            Process CLI flags/environment variables on <Code>sms-gateway serve</Code>, set at deploy
            time — not database rows, and out of this console&apos;s reach by design.
          </p>
        </div>
      </div>
    </ScreenStack>
  );
}
