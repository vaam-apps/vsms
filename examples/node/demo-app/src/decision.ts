/**
 * The one place that decides pass/fail for `demo-app`'s own run —
 * deliberately extracted out of `main()` and kept pure (no I/O, no
 * clock, no network) so it can be tested directly, and so it cannot
 * quietly become something other than what it says it is.
 *
 * Why this matters more than it looks like it should: a sabotage pass
 * against the previous, inline version of this logic (`delivered &&
 * verifiedCount >= 1` inverted to `delivered || verifiedCount === 0`)
 * passed all four of this package's own tests and both CI gates —
 * neither `pnpm test` nor the cross-language signature tests exercise
 * the exit-code decision at all, only the signature-verification
 * mechanics underneath it. `decision.test.ts` is what closes that gap:
 * it asserts this function's own output directly, for all four
 * (delivered × verified) quadrants, with no server, no HTTP, no SDK.
 */

export interface DecisionInput {
  /** Did the message reach the terminal `delivered` state? */
  delivered: boolean;
  /** How many received webhooks verified their signature. */
  verifiedCount: number;
  /** How many webhooks were received at all (verified or not). */
  eventCount: number;
}

export interface Decision {
  exitCode: 0 | 1;
  /**
   * Empty on success. On failure, one entry per failed condition —
   * `main()` may enrich these with extra runtime detail (the final
   * observed message state, say) before printing, but the text here is
   * what `decision.test.ts` asserts against directly.
   */
  reasons: string[];
}

/**
 * Success is exactly "reached delivered AND at least one webhook for it
 * verified" — both halves are required, not either/or. Getting this
 * backwards (`||` instead of `&&`, or comparing `verifiedCount` the
 * wrong way) is precisely the bug class this extraction exists to catch
 * — see this module's own doc comment above.
 */
export function decide({ delivered, verifiedCount, eventCount }: DecisionInput): Decision {
  if (delivered && verifiedCount >= 1) {
    return { exitCode: 0, reasons: [] };
  }

  const reasons: string[] = [];
  if (!delivered) {
    reasons.push("message never reached delivered");
  }
  if (verifiedCount === 0) {
    reasons.push(
      eventCount === 0
        ? "no webhook was received at all"
        : `${eventCount} webhook(s) were received but NONE verified their signature — check that WEBHOOK secret matches the seeded WebhookEndpoint`,
    );
  }
  return { exitCode: 1, reasons };
}
