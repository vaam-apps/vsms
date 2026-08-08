//! Assembling the generated router.

use cratestack::axum::http::Method;
use cratestack::axum::middleware::from_fn_with_state;
use cratestack::axum::Router;
use cratestack_codec_json::JsonCodec;

use crate::auth::GatewayAuth;
use crate::pepper::HashPepper;
use crate::procedures::Procedures;
use crate::rbac::{enforce_route_permission, RbacState, RoutePermission};
use crate::schema;

/// #24's concrete write-route anchor, for #25's gate test to target (see
/// `docs/architecture.md` §5.2's role table: `operator` has
/// `provider:read/update`, `developer` has no `provider:*` permission at
/// all). The permission literal enforced here is `provider:update` —
/// §5.2's own name for it, not the informal "provider:write" phrasing the
/// M1 gate description (§ milestone table) uses for the route in prose.
/// Caught live by Lightbridge's review of #112: an earlier draft of this
/// constant checked the literal `"provider:write"`, which appears nowhere
/// in §5.2's actual vocabulary — it would have silently and permanently
/// denied a legitimate `operator` token the moment a role-bearing token
/// exists to present it, since that token's `perms` would carry
/// `provider:update`, never `provider:write`.
///
/// Picked over a procedure because none of the seven exists for this —
/// `Provider.update`'s own generated route (`PATCH /providers/{id}`,
/// confirmed via `route_table()`) is the natural CRUD write action the
/// design doc's role table is describing. Its own `@@allow` (`schema.cstack`):
/// `hasRole('owner') || hasRole('admin') || hasRole('operator')` — no
/// `hasRole('app')`, unlike `sendMessage`'s procedure-level `@allow`. Combined
/// with `GatewayAuth` only ever minting `role: "app"` or `role: "system"`
/// (see its own doc — this deployment has no human-login path yet, #23/#24/
/// #25's own tracked scope cut), Layer 1 alone already refuses every token
/// this deployment can currently issue on this route, developer-shaped or
/// not. This gate is real and tested (see `rbac.rs`'s own tests plus
/// `tests/rbac_layer2_live_postgres.rs`) but is defense in depth today, not
/// yet the thing a live caller actually bounces off — it becomes load-
/// bearing the moment a role-bearing token exists to test the *positive*
/// case against, which is why this constant, not a bespoke one-off, is what
/// #25 should extend.
const PROVIDER_WRITE_ROUTES: &[RoutePermission] = &[RoutePermission {
    method: Method::PATCH,
    path: "/providers/{id}",
    permission: "provider:update",
}];

/// Build the HTTP surface: generated model CRUD plus the seven procedures.
///
/// `auth` validates every request against the OP's own published JWKS —
/// see [`GatewayAuth`]'s own documentation for what it accepts and why.
///
/// `pepper` is real secret material (#134) — see `pepper.rs`'s module doc
/// for the scheme and the rotation consequence. `sms-gateway serve`'s CLI
/// parsing rejects a missing or too-short pepper before this is ever
/// called, so this function itself never needs to.
///
/// Wrapped in [`enforce_route_permission`] (Layer 2, #24) gating
/// [`PROVIDER_WRITE_ROUTES`] — every other route passes through unchanged;
/// see that constant's own doc and `rbac.rs`'s module doc for the full
/// reasoning.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to.
pub fn router(db: schema::Cratestack, auth: GatewayAuth, pepper: HashPepper) -> Router {
    let rbac_state = RbacState {
        auth: auth.clone(),
        requirements: PROVIDER_WRITE_ROUTES,
    };
    schema::axum::router(db, Procedures::new(pepper), JsonCodec, auth)
        .layer(from_fn_with_state(rbac_state, enforce_route_permission))
}

/// Every route the schema generated, for `sms-gateway routes`.
///
/// Enumerated rather than hardcoded because `pluralize()` is naive
/// (`ends_with('s') ? +"es" : +"s"`) and there is no `@@map`, so the only
/// reliable statement of a path is the one the macro emitted.
#[must_use]
pub fn route_table() -> Vec<(&'static str, &'static str)> {
    schema::axum::ROUTE_TRANSPORTS
        .iter()
        .map(|route| (route.method, route.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_generated_a_route_surface() {
        let routes = route_table();
        assert!(!routes.is_empty(), "no routes were generated");
    }

    #[test]
    fn messages_are_reachable_under_a_pluralised_path() {
        // Guards the `pluralize()` trap: if the emitter's naive pluralisation
        // ever changes, this fails here rather than in the admin console.
        let routes = route_table();
        assert!(
            routes.iter().any(|(_, path)| path.contains("messages")),
            "expected a /messages path, got: {routes:?}"
        );
    }

    // Async because a lazy pool still spawns a background task on construction,
    // so `connect_lazy` panics with "this functionality requires a Tokio
    // context" outside a runtime — even though it never opens a connection.
    #[tokio::test]
    async fn router_builds_without_a_live_database() {
        let pool = cratestack::sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/none")
            .expect("a lazy pool only parses the URL");
        let db = schema::Cratestack::builder(pool).build();
        let auth = GatewayAuth::new(
            db.clone(),
            "https://auth.invalid/jwks.json".to_owned(),
            "https://auth.invalid".to_owned(),
        );
        let pepper = HashPepper::new("a".repeat(crate::pepper::MIN_PEPPER_BYTES)).unwrap();
        let _router = router(db, auth, pepper);
    }
}
