// Dumb component (R6): the pairing screen's markup. One question with one
// answer, like the rest of the product: the QR, the three steps that complete
// it, and nothing to fill in. No password, no code to type and no session: this
// browser made a key it cannot export, shows it as a QR, and a vaam admin phone
// that is already signed in vouches for it.

import { InlineBanner, Spinner } from "@vaam-apps/ui";
import { QrCode } from "@/components/qr-code";
import type { PairScreen } from "@/lib/pair-flow";

export interface PairViewProps {
  screen: PairScreen;
  /** Minutes this computer's clock is off by, when that is enough to matter. */
  clockSkewMinutes?: number | undefined;
  /** Why the last attempt could not start, if it could not. */
  failure?: string | undefined;
  /** True when the previous key of this browser ended (expired or revoked). */
  replacing?: boolean | undefined;
}

const STEPS = [
  "Open the vaam admin app on your phone and choose Pair a console.",
  "Scan this code and check the browser named above is yours.",
  "Confirm with your fingerprint. This page opens the console by itself.",
];

export function PairView({ screen, clockSkewMinutes, failure, replacing }: PairViewProps) {
  return (
    <main className="flex min-h-dvh items-center justify-center p-6">
      <div className="flex w-full max-w-md flex-col items-center gap-6 text-center">
        <div className="flex flex-col gap-2">
          <h1 className="font-display text-headline-medium text-foreground">Pair this console</h1>
          <p className="text-body-medium text-muted-foreground">
            This console has no password. A vaam admin phone vouches for the key this browser makes,
            and the key signs every request.
          </p>
        </div>

        {replacing === true && (
          <InlineBanner variant="warning">
            This console&rsquo;s previous key has expired or was revoked. Pairing makes a new one.
          </InlineBanner>
        )}
        {clockSkewMinutes !== undefined && (
          <InlineBanner variant="danger">
            This computer&rsquo;s clock is about {Math.abs(clockSkewMinutes)} minutes{" "}
            {clockSkewMinutes > 0 ? "ahead of" : "behind"} the server&rsquo;s. The gateway refuses
            anything signed more than five minutes off its own time, so pairing will not work until
            the clock is corrected.
          </InlineBanner>
        )}
        {failure !== undefined && <InlineBanner variant="danger">{failure}</InlineBanner>}

        {screen.kind === "waiting" ? (
          <>
            <QrCode
              text={screen.qr.text}
              label="Pairing code for this console"
              className="size-64 rounded-xl border border-edge sm:size-72"
            />
            <p className="text-body-medium">
              <span className="text-muted-foreground">Browser: </span>
              <strong data-testid="pair-label">{screen.label}</strong>
            </p>
            <p
              role="status"
              aria-live="polite"
              className="flex items-center gap-2 text-label-large"
            >
              <Spinner />
              {screen.reachable
                ? "Waiting for your phone"
                : "Waiting for your phone. The gateway can't be reached from here right now."}
            </p>
          </>
        ) : (
          <div role="status" className="flex items-center gap-3 text-muted-foreground">
            <Spinner />
            <span>
              {screen.kind === "paired" ? "Paired. Opening the console" : "Preparing a key"}
            </span>
          </div>
        )}

        <ol className="flex list-decimal flex-col gap-2 pl-5 text-left text-body-medium">
          {STEPS.map((step) => (
            <li key={step}>{step}</li>
          ))}
        </ol>

        <details className="w-full text-left text-body-small text-muted-foreground">
          <summary className="cursor-pointer text-label-large">No vaam admin phone?</summary>
          <div className="flex flex-col gap-2 pt-2">
            <p>
              An operator with access to the gateway can attach this browser&rsquo;s key to an
              existing account from the command line. This page notices on its own once the gateway
              accepts the key.
            </p>
            {screen.kind === "waiting" && (
              <pre
                data-testid="pair-command"
                className="overflow-x-auto rounded-lg bg-base-200 p-3 text-body-small text-foreground"
              >
                {`sms-gateway pair-console-key --user <email> --public-key ${screen.publicKey}`}
              </pre>
            )}
          </div>
        </details>
      </div>
    </main>
  );
}
