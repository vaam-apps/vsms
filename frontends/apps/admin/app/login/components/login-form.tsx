import { Button, InlineBanner, Input, Label } from "@vsms/ui";

/**
 * The login screen's markup and classes — the dumb layer for
 * `frontends/apps/admin/app/login/page.tsx`.
 *
 * Route-local rather than shared (`@vsms/ui`), per R6's own test: no second
 * route would plausibly render a login form, and this encodes exactly one
 * screen's shape.
 *
 * **This stays a plain `<form method="post">` with no client JS.** That is
 * not a styling decision and must survive any future redesign: `page.tsx`'s
 * own module doc explains that the browser never holds a token, and
 * `middleware.ts` has already minted the `vsms_oidc_txn` cookie
 * (state/nonce/PKCE) before this renders. Turning this into a `"use
 * client"` component with a `fetch` would move the credential handoff into
 * script the page does not need.
 *
 * Two things were genuinely wrong here before the R6 sweep, not just
 * misplaced:
 *
 *  1. **The classes were light-theme.** `bg-red-50`, `border-red-300`,
 *     `bg-black`, `text-white` — raw Tailwind palette on a console the
 *     redesign locked to dark-only. They now use the same semantic tokens
 *     every other screen does (`bg-state-danger-bg`, `border-edge`,
 *     `text-foreground`), so this screen follows the theme instead of
 *     fighting it.
 *  2. **It hand-rolled `<input>`, `<label>` and `<button>`** while
 *     `@vsms/ui` already exported `Input`, `Label` and `Button`. The
 *     original file's own doc comment claimed it "uses `@vsms/ui`'s
 *     existing primitives where they fit" — it did not use any. Now it
 *     does, which is also what makes the daisyUI focus/disabled states and
 *     the `--radius-field` token reach this screen for free.
 *
 * `autoComplete="username"`/`"current-password"` are kept verbatim: they
 * are what let a password manager fill this form, and they are easy to drop
 * silently while moving markup.
 */
// `?: string | undefined`, not `?: string` — this workspace sets
// `exactOptionalPropertyTypes: true`, so an explicitly-passed `undefined`
// is not assignable to a plain optional. `loginErrorMessage` returns
// `string | undefined` by design (that is how "render no alert at all" is
// expressed), so the caller passes `undefined` rather than omitting the
// prop.
export function LoginForm({ errorMessage }: { errorMessage?: string | undefined }) {
  return (
    <main className="mx-auto flex min-h-screen max-w-sm flex-col justify-center gap-6 px-4">
      <h1 className="font-semibold text-foreground text-xl">Sign in</h1>

      {errorMessage !== undefined && (
        <InlineBanner variant="danger" className="text-foreground">
          <span role="alert">{errorMessage}</span>
        </InlineBanner>
      )}

      <form method="post" action="/api/auth/login" className="flex flex-col gap-4">
        <div className="flex flex-col gap-1">
          <Label htmlFor="login-email">Email</Label>
          <Input id="login-email" type="email" name="email" required autoComplete="username" />
        </div>

        <div className="flex flex-col gap-1">
          <Label htmlFor="login-password">Password</Label>
          <Input
            id="login-password"
            type="password"
            name="password"
            required
            autoComplete="current-password"
          />
        </div>

        <Button type="submit">Sign in</Button>
      </form>
    </main>
  );
}
