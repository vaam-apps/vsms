-- 0009_drop_openid_provider / up.sql
--
-- VS-4: deletes the embedded OpenID Provider and the human OAuth plane. Since
-- VS-3 every route opens a COSE_Sign1 envelope from a registered key
-- (`client_keys`, `console_keys`) and no code reads a token, a password or an
-- upstream id_token, so the five tables below have no reader and no writer:
--
--   oauth_clients       the OP's client registry (private_key_jwt / public)
--   oauth_signing_keys  the OP's RS256 signing keys (private halves, PEM)
--   client_assertions   spent private_key_jwt `jti`s
--   user_credentials    Argon2id password hashes
--   sso_transactions    in-flight Keycloak sign-ins (state, nonce, PKCE verifier)
--
-- `federated_links` stays: console pairing finds or makes a person through it
-- (issuer `vaam`), keyed on `(issuer, subject)`.
--
-- 0001_init and 0002_bootstrap (and 0005 for `sso_transactions`) have already
-- been applied to deployed databases and `sms-migrate` records migrations by
-- name, so they cannot stop creating these tables; this forward migration
-- removes them, and `cargo xtask migrations-current` knows that
-- (DROPPED_BY_LATER in .xtask/src/migrations_current.rs): it removes each
-- table's statements from the committed `0001_init` before comparing it with
-- the generator, and checks that this file drops every one.
--
-- Dropping a table takes its indexes, CHECK constraints, defaults, foreign
-- keys (`oauth_clients.app_client_id`, `user_credentials.user_id`) and
-- `touch_updated_at` triggers with it. `cratestack_audit` rows written about
-- these models are an append-only history and stay.
--
-- Irreversible by intent: the password hashes and the private signing keys
-- must not come back from a `down`. See down.sql.

DROP TABLE client_assertions;

DROP TABLE oauth_clients;

DROP TABLE oauth_signing_keys;

DROP TABLE sso_transactions;

DROP TABLE user_credentials;
