Layer 2 (#24, §5.1 of the design doc): permission/scope enforcement that
runs in Rust *before* a procedure body, or a generated route's handler,
ever executes — on top of whatever Layer 1's `@@allow` already decided
for the caller's role.

One primitive, [`require_permission`], two call sites:

- a procedure calls it directly, at the top of its body, before doing
  anything else (see `Procedures::send`, gated on `"sms:send"`) — the
  "in procedures" half of §5.1's "in procedures and a Tower layer";
- [`enforce_route_permission`] wraps it in an axum middleware for
  *generated* CRUD routes, which have no procedure body to call it
  from — the "a Tower layer" half. `router::PROVIDER_WRITE_ROUTES` is the
  one route this milestone wires it onto (`PATCH /providers/{id}`,
  gated on the `provider:update` permission — §5.2's own name for it,
  not the "provider:write" phrasing the milestone-gate prose uses for
  the route itself — left in place for #25); a second route needs only
  another entry in that slice, not a new mechanism.

# Fail closed, per §5.2's own words

*"An omitted `scope` yields `scope: None`, which your check must treat
as denial."* [`require_permission`] extends that same rule to `perms`:
a caller with neither claim present — or present but not containing the
literal required — is denied. There is no default-allow path.

# Layer 1 stays the real perimeter

§5.1's invariant: *"every permission checked in layer 2 sits behind a
role gate in layer 1 that is at least as restrictive. Layer 2 narrows;
it never widens."* Nothing here ever grants an operation Layer 1's own
`@@allow` would deny — `enforce_route_permission` only ever adds a
401/403 *before* the generated router's own policy check, never a
bypass of it. See `PROVIDER_WRITE_ROUTES`'s own doc in `router.rs` for the
concrete consequence, which VS-3 changed: a paired console key now makes a
role-bearing (`user`) principal a real caller of these routes. A key can be an
app's (`role "app"`, scopes only) or a person's (that person's role, with the
role's permissions as `perms`), so `Provider.update`'s `@@allow` admits an
`owner`, `admin` or `operator` at Layer 1 and `provider:update` is what Layer 2
then asks of them. The gate is no longer defense in depth for the `PATCH
/providers/{id}` route: an `admin` who holds a role without the permission is
refused here, which Layer 1 alone would let through.

# A read that follows a permission: a claim, with Layer 2 in front of it

A policy lists roles (`hasRole` takes one literal) and cannot read `Role.permissions`, so a
route gate alone cannot bind a read that reaches the same rows by another door: `?include=`,
a relation filter and a nested include read the related model under *its own* policy. Reading
a message therefore asks twice. The policy asks for the `messageRead` claim
(`auth::Grants`, set from the same `perms` and `scope` this module checks), which binds every
door. `router::MESSAGE_READ_ROUTES` asks `sms:read` as well, in front of it, so the direct
routes answer a 403 that names the permission and not an empty page. A requirement on a
`GET` also gates the `HEAD` ([`RoutePermission::applies_to`]): `HEAD` is answered by the
`GET` handler with the full `Content-Length`.

# A second kind of Layer-2 middleware: guarding the *value* written

`enforce_route_permission` asks "may this caller use this route at all". Some
rules are about the **value** a permitted caller writes, which a Layer-1
`UPDATE … WHERE <policy>` cannot see (it checks the stored row, never the new
one). [`owner_grant`](crate::owner_grant) is the first: only an `owner` may set
a `User`'s `roleKey` to `owner`. It reuses `authenticate_request` (the same
re-authentication this module's gate runs) and follows the same rule — it only
ever adds a refusal in front of Layer 1, never a bypass of it.

<!-- markdownlint-disable-file MD041 -->
