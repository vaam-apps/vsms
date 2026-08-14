import type { ReactNode } from "react";
import { cn } from "../../lib/cn";

/**
 * Inline monospace emphasis for a literal term — a config key, a role key,
 * a job kind, a model or field name.
 *
 * **Not `IdDisplay`, and the distinction is load-bearing.** `IdDisplay` is
 * documented as specifically for `cs_cuid()` values: it truncates to seven
 * characters with no middle ellipsis and offers a copy button, both of
 * which are correct for an opaque 23-character id and wrong for anything
 * else. Three separate agents independently checked their own sites
 * against `schemas/vsms.cstack` and reached the same conclusion — none of
 * these are `Cuid` (`AppClient.clientId` is a plain bounded `String`,
 * `Role.key` is `@regex`-constrained, a route predicate is an operator or
 * prefix, `providerMessageRef` is an *external* provider's id) — so
 * routing them through `IdDisplay` would truncate values that must be read
 * in full.
 *
 * This exists because `font-mono text-foreground` was the most-repeated
 * class string left after the R6 factorization (31 occurrences), and
 * because three route groups had already each created a byte-identical
 * local `Code` component for it. Those three copies are exactly the
 * "same shape, different name" duplication that produced two
 * `ScreenHeader`s and two `MESSAGE_CLASSES` earlier in this effort — the
 * agents that wrote them flagged the risk themselves and could not fix it,
 * because a cross-route duplicate is not a route-scoped change.
 *
 * Renders a `<span>`, not a `<code>`: several call sites sit inside a
 * `<dd>` or a sentence where `<code>` would add semantics the content does
 * not have (a role key in prose is emphasis, not a code sample). A caller
 * that genuinely wants `<code>` semantics should say so rather than have
 * this component guess.
 */
export function Code({
  children,
  className,
}: {
  children: ReactNode;
  className?: string | undefined;
}) {
  return <span className={cn("font-mono text-foreground", className)}>{children}</span>;
}
