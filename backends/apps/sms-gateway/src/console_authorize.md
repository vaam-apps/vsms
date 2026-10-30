The console's authorization request, and the checks both ways of signing in make on it
(VSA-4c).

`POST /login` (a password, `login.rs`) and `GET /login/sso/start` (single sign-on,
`sso.rs`) are two ways to decide who a person is, and they end in the same place:
the library's own `handle_authorize`, issuing the console a code. What is asked of
the console before that is the same, so it is written once.

- [`ConsoleAuthorize`] is the eight OAuth parameters, `snake_case`, with no unknown
  field. A sign-in carries it in a JSON body, an SSO start in a query string, and an
  SSO transaction stores it as JSON (`SsoTransaction.consoleRequest`) to replay once
  the person comes back from the identity provider.
- [`ConsoleAuthorize::malformed`] is what the request alone proves wrong: a method
  other than `S256`, a challenge that is not 43 to 128 base64url characters, a
  `redirect_uri` the console is not registered with. Made before anything is
  spent - a bucket token, an Argon2 hash, a database row.
- [`ConsoleAuthorize::authorize_request`] builds the library's `#[non_exhaustive]`
  request through its `Deserialize`, which is why a test pins that the optional
  fields survive.
- [`redirect_carries_an_error`] tells the library's error redirect from a code.
  Neither route forwards one: both answer `400`. (The first SSO broker forwarded an
  error redirect to the browser; VSA-4a's rule for the password path - the console is
  never handed an error as though it were a sign-in - holds for both now.)

What differs between the two stays in each route: the password sign-in runs the real
`handle_authorize` with the real store once the password is right, while SSO start
runs it against a throwaway store as a **dry run** (nothing it issues is kept)
before it has sent the person anywhere.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
