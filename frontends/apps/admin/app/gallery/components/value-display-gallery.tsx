"use client";

// Route-local (R6). The value-formatting primitives added when this
// package was generalised — money, masked secrets, copy, and the KPI
// tile. vsms itself only uses `Money` for `costXaf`; the others are
// mounted here so the gallery keeps covering every export.

import { CopyButton, MaskedValue, Money, StatTile } from "@vaam-apps/ui";
import { GallerySwatch } from "./gallery-swatch";
import { Section } from "./section";

export function ValueDisplayGallery() {
  return (
    <Section
      title="Money, masked values, copy, stat tiles"
      description="Money takes an integer count of minor units and derives the exponent from Intl, so the three-decimal currencies every hand-written table forgets (KWD, BHD) are right, and an i64 past 2^53 formats exactly rather than losing its last unit to a float division."
    >
      <div className="flex flex-col gap-4">
        <GallerySwatch label="Money — zero-decimal, two-decimal, three-decimal, and an amount past Number.MAX_SAFE_INTEGER">
          <div className="flex flex-wrap items-center gap-6">
            <Money amount={500_000} currency="XAF" />
            <Money amount={1250} currency="USD" />
            <Money amount={1250} currency="KWD" />
            <Money amount="9007199254740993" currency="USD" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="Money — display modes, and the signed tone for a ledger delta">
          <div className="flex flex-wrap items-center gap-6">
            <Money amount={1250} currency="USD" display="symbol" />
            <Money amount={1250} currency="USD" display="none" />
            <Money amount={1250} currency="USD" tone="signed" signDisplay="always" />
            <Money amount={-1250} currency="USD" tone="signed" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="MaskedValue — a screen-share control, not a security boundary: the value already crossed the wire">
          <div className="flex flex-col gap-3">
            <MaskedValue
              value="whsec_9f2c41b6a7e84d15b3c0a8e7f2d9146c8b0e3a75d2f6194c8a3b7e05d19f2c4a6"
              prefix={6}
              reveal={4}
              label="signing secret"
            />
            <MaskedValue value="JBSWY3DPEHPK3PXP" reveal={0} label="TOTP secret" />
            <MaskedValue value="+237677123456" reveal={3} revealable={false} label="payer number" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="CopyButton — always copies the full machine-readable value, never what is displayed">
          <div className="group flex flex-wrap items-center gap-2 font-mono text-caption">
            <span>pi_3QxR7w2eZvKYlo2C1a2b3c4d</span>
            <CopyButton value="pi_3QxR7w2eZvKYlo2C1a2b3c4d" />
          </div>
        </GallerySwatch>
        <GallerySwatch label="StatTile — one headline number with the denominator that makes it mean something">
          <div className="grid gap-3 sm:grid-cols-3">
            <StatTile label="Delivered (24h)" value="12,481" caption="98.2% of 12,710 terminal" />
            <StatTile
              label="Uncertain"
              value="37"
              tone="uncertain"
              caption="Outcome never learned — not retried"
            />
            <StatTile
              label="Spend (24h)"
              value={<Money amount={318_420} currency="XAF" display="none" />}
              caption="XAF, across all providers"
            />
          </div>
        </GallerySwatch>
      </div>
    </Section>
  );
}
