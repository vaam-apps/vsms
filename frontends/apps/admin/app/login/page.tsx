/**
 * The console's login form (#194). A plain HTML `<form method="post">` to
 * `/api/auth/login` — no client-side JS, no fetch, no token ever visible
 * to a browser script. `frontends/apps/admin/middleware.ts` has already minted and set
 * the `vsms_oidc_txn` cookie (state/nonce/PKCE) before this component
 * ever renders — see that file's own module doc for why it has to happen
 * there rather than here.
 *
 * R6: this page composes and nothing else. Markup and classes live in
 * `./components/login-form`; the `?error=` code mapping lives in
 * `./login-errors` (pure, tested). Before the sweep this file carried all
 * three, plus a light-theme palette on a dark-only console — see the form
 * component's own doc for what that actually broke.
 */
import { LoginForm } from "./components/login-form";
import { loginErrorMessage } from "./login-errors";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export default async function LoginPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  return <LoginForm errorMessage={loginErrorMessage(params.error)} />;
}
