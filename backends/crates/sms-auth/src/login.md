Password verification for human console logins (#194).

# Why a password at all — read this before copying the pattern elsewhere

§4.3 of the design doc names the shape ("Authorization code + PKCE
against `sms-auth`, single OIDC client `sms-console`") but never decided
*how* a human proves who they are before that code is issued — #97/#98
cut the whole human-login path because no console existed yet to need
it, and #194 is the ticket that closes that cut. Two paths were weighed:

1. Delegate authentication to an external OIDC `IdP`, with `sms-auth`
   acting as both OP (to `sms-console`) and RP (to that `IdP`) — a
   broker/federation pattern. Rejected for this PR: it needs a second,
   equally security-sensitive OIDC client implementation (discovery,
   its own token validation, its own callback), an actual external `IdP`
   this deployment can point at (none is decided — see AGENTS.md's
   "Open questions blocking later milestones" list, none of which this
   PR resolves), and — critically for a change of this sensitivity —
   nothing in this sandbox to prove it against end to end the way this
   repo's own testing convention (a fake for every external dependency:
   `sms-fake-orange`) demands. It remains the better long-term answer
   for a multi-app or multi-tenant future; #194's own issue text left it
   open rather than deciding it, and this PR doesn't decide it either —
   it is *ruled out for now*, not closed.
2. Local password authentication, entirely within this system, backed
   by Argon2id. **Chosen.** It needs nothing this deployment doesn't
   already have, it's fully testable against a real Postgres with no
   external dependency, and it matches this codebase's own "no Redis,
   no external services, Postgres is the only coordination mechanism"
   posture (AGENTS.md, "Stack, pinned").

**Say it plainly, because the issue that created this file demanded it
be said loudly: this is a new security surface.** This system now
stores password material — hashed, never plaintext, never logged, never
returned by any API response (see [`schema::UserCredential`]'s own
schema comment for why it is a *separate* model from `User` rather than
a field on it) — but it did not before. The PR that introduces this
file's Risk Assessment section says so explicitly rather than letting a
reviewer discover it by reading the diff.

# Why `UserCredential`, not a field on `User`

See [`schema::UserCredential`]'s own doc comment in `schema.cstack` — the
short version is §2.0's "no field-level read masking" constraint:
`@sensitive` redacts audit snapshots only, never an HTTP response body,
so a password hash living on `User` itself would come back verbatim from
`GET /users/{id}` to every role `User.read` admits. A separate model
restricted to `hasRole('system')` on every action is the only mechanism
this framework offers for "never returned by the API at all."

# Timing and user enumeration

[`authenticate_user`] always calls into Argon2 verification — against a
fixed dummy hash when no matching, active `User`/`UserCredential` pair
exists — so a caller cannot distinguish "no such user" from "wrong
password" by response latency. Both fail with the identical
[`LoginError::InvalidCredentials`], carrying no detail a caller could
use to enumerate accounts.

# #52/#58: hashing itself moved to `sms_core::password`

`hash_password`/`verify_password` used to live in this file. They now
live in [`sms_core::password`] — the console's own `provisionUser`
procedure (#58) needs to hash a freshly generated one-time password from
`sms-api`, which cannot depend on this crate (`sms-auth` depends on
`sms-api`, not the reverse). See that module's own doc for the full
reasoning; this file now calls it exactly like any other caller.
