A person's password sign-in (VSA-4a, restored from `29f93c4^`): look the account
up, verify the password, resolve the role; and, once the caller has issued a code,
stamp `lastLoginAt`.

This system stores password hashes, which the signed transport did not: new
security surface, not a footnote. Local Argon2id rather than federation, because
federation needs a second security-sensitive `OpenID` client and an identity
provider nobody has chosen (VSA-4c brings the Keycloak broker back beside this
path, it does not replace it).

# What `authenticate_user` guarantees

- **A real Argon2 verification runs on every call**, against the stored hash or,
  when no usable account matches, against a dummy hash made by this process's own
  [`hash_password`] (`dummy_hash`). "No such account", "no password set on it",
  "inactive", "soft-deleted" and "wrong password" cost the same work and return the
  same [`LoginError::InvalidCredentials`]. The dummy is made at first use from a
  **random throwaway password that is dropped at once**, so there is no fixed value
  in the source that could be a password. A hand-typed PHC literal would risk
  failing to parse, which makes the missing-account branch faster than a real
  check and reopens the side channel; nobody knows the password behind this one,
  so a unit test holds it to *the same variant and cost parameters as a real hash*
  (`hash_cost`), and the live suite holds the clock to it
  (`an_unknown_email_takes_as_long_as_a_wrong_password`).
- **The database work is the same on every refusal.** The `User` read and the
  `UserCredential` read both run whether or not an account matched; with none, the
  second looks for a `User.id` no row has. An existing account used to cost one
  statement more, a signal left once the hashing was equal. The live suite counts
  the statements (sqlx emits one `sqlx::query` event each) for an unknown email, a
  wrong password, an inactive account and an account with no password, and requires
  the same number.
- **It reads under `system_context(`[`LOGIN_ACTOR`]`)`** (`sms-gateway:login`): no
  person exists yet to authorize the read, so the lookups of `User`, `UserCredential`
  and `Role` run as the internal `system` principal, which the `hasRole('system')`
  clause on each admits. The actor lands in `cratestack_audit.actor.sub`.
- **It does not stamp `lastLoginAt`; [`stamp_last_login`] does, when the caller
  says a code was issued.** A right password is not yet a sign-in: the gateway still
  has an authorization request for the library to accept, and a person whose request
  it refuses signed nobody in. The stamp is a write to `User` under the same
  context, and the one reason `User.update` carries `|| hasRole('system')` again:
  the clause was removed in VS-4 with the login and is restored with it. Without it
  the write affects zero rows and the login still succeeds, so the stamp's failure
  is logged at `warn` and only a test that reads the column back can see it
  (`login_live_postgres`). The stamp is under the `@version` the account was read at
  ([`AuthenticatedUser::last_login_stamp`]): a concurrent edit that moved it skips
  the stamp, never the login.
- **It does not look at what the role is.** A `Role` keyed `system` or `app` is
  refused twice already, independently of this module: the database CHECK
  (`roles_key_not_reserved_check`) will not create one, and `sms_api::signers`
  refuses one at the point of use (`reserved_role_live_postgres` proves the second
  with the first removed). A code issued to such a person opens nothing, because
  the API re-resolves the role from rows on every request. A third refusal here
  would be untestable without a thirteenth raw-SQL exception (R1), so there is
  none.
- **An outage is not a refusal** ([`LoginError::Unavailable`]): a database error is
  reported as one (the gateway answers 503), not as "invalid email or password".
  It reveals nothing about whether the account exists, because it does not depend
  on it.

# What it does not do

It does not rate-limit (the gateway does, per email address and in aggregate,
*before* calling this: `login.rs`), does not check the `client_id` (the gateway
does), and does not issue a code: that is `handle_authorize`, run by the gateway
with the [`AuthenticatedUser`] this returns as the code's identity. Role and
permissions are **not** copied into the code or the token; the API re-resolves them
from rows on each request (VSA-4b), so a deactivation takes effect within one
cache window.

<!-- markdownlint-disable-file MD041 -- rustdoc sidecar: include_str!'d as module docs, so it opens with prose, not a heading; at the end so the first paragraph stays rustdoc's summary line -->
