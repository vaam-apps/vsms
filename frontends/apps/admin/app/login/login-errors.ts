/**
 * Maps the `?error=` code `/api/auth/login` redirects back with onto text a
 * person can act on.
 *
 * Extracted from `page.tsx` per R6 ("a view file contains no helpers
 * either" — a mapping object belongs beside the data it maps, not inlined
 * in a screen). Pure, so it carries a test, per the same section.
 *
 * The codes are not invented here. They are exactly what
 * `frontends/apps/admin/app/api/auth/login/route.ts` emits, and the
 * `unknownFallback` case is load-bearing rather than defensive padding:
 * that route can also redirect with an error this table has never seen,
 * and rendering an empty alert would tell the user their password worked.
 */

/** Every `?error=` code the login route can redirect with. */
export const LOGIN_ERROR_MESSAGES: Record<string, string> = {
  invalid_credentials: "Incorrect email or password.",
  invalid_request: "The login request was malformed. Try again.",
  expired: "Your login attempt expired. Try again.",
};

/** Shown for a code not in the table — see the module doc for why this is
 * not merely defensive. */
export const UNKNOWN_LOGIN_ERROR = "Login failed.";

/**
 * Resolves a raw `searchParams.error` value to display text.
 *
 * Takes the raw `string | string[] | undefined` rather than a
 * pre-narrowed string on purpose: Next hands a repeated query parameter
 * (`?error=a&error=b`) back as an array, and doing that narrowing in the
 * page meant the page carried logic. First value wins, matching how the
 * page behaved before this was extracted.
 *
 * Returns `undefined` for "no error", which is what lets the caller decide
 * to render nothing at all rather than an empty alert region.
 */
export function loginErrorMessage(raw: string | string[] | undefined): string | undefined {
  const code = Array.isArray(raw) ? raw[0] : raw;
  if (code === undefined) {
    return undefined;
  }
  return LOGIN_ERROR_MESSAGES[code] ?? UNKNOWN_LOGIN_ERROR;
}
