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
concrete consequence: this deployment's `GatewayAuth` never issues a
caller any role but `"app"` or `"system"` (see its own doc), and
`Provider.update`'s `@@allow` admits neither — so Layer 1 alone already
fully closes that route to every token this deployment can mint today.
Layer 2's gate on it is real, tested, and load-bearing the moment a
role-bearing (human) token exists; until then it is defense in depth,
not the thing actually stopping a live request.
