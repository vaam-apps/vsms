//! Assembling the generated router.

use cratestack::axum::Router;
use cratestack_codec_json::JsonCodec;

use crate::auth::DenyAll;
use crate::procedures::Procedures;
use crate::schema;

/// Build the HTTP surface: generated model CRUD plus the seven procedures.
///
/// The auth provider is [`DenyAll`] until milestone 1 — see its documentation.
// No `#[must_use]`: axum's `Router` already carries one, and doubling it is
// what `clippy::double_must_use` objects to.
pub fn router(db: schema::Cratestack) -> Router {
    schema::axum::router(db, Procedures::new(), JsonCodec, DenyAll)
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
        let _router = router(schema::Cratestack::builder(pool).build());
    }
}
