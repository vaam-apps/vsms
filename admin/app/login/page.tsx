/**
 * The console's login form (#194). A plain HTML `<form method="post">` to
 * `/api/auth/login` — no client-side JS, no fetch, no token ever visible
 * to a browser script. `admin/middleware.ts` has already minted and set
 * the `vsms_oidc_txn` cookie (state/nonce/PKCE) before this component
 * ever renders — see that file's own module doc for why it has to happen
 * there rather than here.
 *
 * Deliberately minimal styling — #194's own scope is the mechanism, not a
 * polished screen (that's #58's own users-and-roles UI, out of scope
 * here). Uses `@vsms/ui`'s existing primitives where they fit rather than
 * hand-rolling new ones, per this project's own "composition over
 * re-implementation" convention.
 */
export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const ERROR_MESSAGES: Record<string, string> = {
  invalid_credentials: "Incorrect email or password.",
  invalid_request: "The login request was malformed. Try again.",
  expired: "Your login attempt expired. Try again.",
};

export default async function LoginPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const rawError = params.error;
  const errorCode = Array.isArray(rawError) ? rawError[0] : rawError;
  const errorMessage =
    errorCode === undefined ? undefined : (ERROR_MESSAGES[errorCode] ?? "Login failed.");

  return (
    <main className="mx-auto flex min-h-screen max-w-sm flex-col justify-center gap-6 px-4">
      <h1 className="text-xl font-semibold">Sign in</h1>
      {errorMessage !== undefined && (
        <p
          role="alert"
          className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800"
        >
          {errorMessage}
        </p>
      )}
      <form method="post" action="/api/auth/login" className="flex flex-col gap-4">
        <label className="flex flex-col gap-1 text-sm">
          Email
          <input
            type="email"
            name="email"
            required
            autoComplete="username"
            className="rounded border px-3 py-2"
          />
        </label>
        <label className="flex flex-col gap-1 text-sm">
          Password
          <input
            type="password"
            name="password"
            required
            autoComplete="current-password"
            className="rounded border px-3 py-2"
          />
        </label>
        <button type="submit" className="rounded bg-black px-3 py-2 text-white">
          Sign in
        </button>
      </form>
    </main>
  );
}
