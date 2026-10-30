-- 0012_restore_openid_provider / up.sql
--
-- VSA-2 of owner ruling R5 (2026-10-27): which ways in a deployment permits is
-- configuration (`auth.methods`, VSA-1), and the default keeps every legacy way,
-- machine tokens and human login among them. 0009_drop_openid_provider (VS-4)
-- dropped the five tables those two ways keep their state in; this migration
-- creates them again, EMPTY, with the shape they last had:
--
--   oauth_clients       the OP's client registry (private_key_jwt / public)
--   oauth_signing_keys  the OP's RS256 signing keys (private halves, PEM)
--   client_assertions   spent private_key_jwt `jti`s
--   user_credentials    Argon2id password hashes
--   sso_transactions    in-flight Keycloak sign-ins (state, nonce, PKCE verifier)
--
-- PERSISTENCE ONLY. Nothing reads or writes these tables yet: the OP and the
-- bearer realm come back in VSA-3, the login and the SSO broker in VSA-4. Until
-- then nothing reads them and they are unreachable: every model carries
-- `@@internal("all")` (no generated route, RPC arm or client stub) on top of
-- policies that admit only the internal `hasRole('system')` context, which no
-- HTTP caller can hold. They are empty unless an operator imported rows by the
-- runbook below; if one did, they hold private signing keys and Argon2id
-- hashes, and nothing reads those either until VSA-3 and VSA-4.
--
-- WHY A NEW MIGRATION AND NOT AN EDIT OF 0009. Migrations are forward-only
-- (owner ruling R5). 0009 has run in deployments and `sms-migrate` records
-- migrations by name, so it stays as written, down.sql and all. This file runs
-- after it, on a database that has just lost the five tables (or, on a new one,
-- has just created and dropped them), and puts them back.
--
-- WHAT A DEPLOYMENT THAT UPGRADES THROUGH 0009 AND THIS FILE LOSES, AND DOES NOT
-- GET BACK. 0009 (VS-4) is in no release tag: the latest, v0.9.0, predates it.
-- So EVERY DEPLOYMENT ON v0.9.0 OR EARLIER runs 0009 and this file together, in
-- one migrate Job, on its next upgrade, and loses the rows. 0009 deleted them,
-- and nothing here can restore what it deleted:
--
--   * every OAuth client (`oauth_clients`): each machine client is provisioned
--     again once VSA-3 lands, with a new keypair;
--   * the OP's signing keys (`oauth_signing_keys`): a new one is minted by
--     `rotate-signing-key` once VSA-3 lands, and a token the old key signed
--     is not verifiable by it;
--   * every password hash (`user_credentials`): each person is given a new
--     password through `provision-user` once VSA-4 lands;
--   * spent client assertions and in-flight SSO sign-ins: ephemeral, and
--     nothing is lost by their absence beyond a sign-in in progress.
--
-- A deployment that wants to keep the first three exports them BEFORE it takes
-- the release that carries 0009, and imports them after this file:
-- docs/runbooks/deployment.adoc, "Keeping the OpenID Provider's rows across 0009
-- and 0012", has the statements, and `a_provider_export_taken_before_0009_is_
-- restored_after_0012` in `restore_openid_provider_live_postgres.rs` runs them.
-- `federated_links`, which 0009 kept, is untouched. `cratestack_audit` rows
-- written about these models are an append-only history and were never dropped.
--
-- THE SHAPE IS THE ORIGINALS', TO THE LETTER. Three migrations built it, and a
-- fresh database that stopped at 0008 had it: the generator's DDL in
-- 0001_init (`oauth_clients`, `oauth_signing_keys`, `client_assertions`,
-- `user_credentials`) and in 0005_sso_federation (`sso_transactions`), and the
-- hand-written remainder in 0002_bootstrap and 0005: the `cs_cuid()` and
-- `now()` defaults, the `touch_updated_at` triggers, the space-delimited list
-- defaults, the signing-key and expiry indexes and the client-registration
-- CHECKs. Each is repeated below in the order the foreign keys allow, with the
-- reasoning its original carried. The foreign keys are the generator's plain
-- `NO ACTION` ones (`oauth_clients.app_client_id` to `app_clients`,
-- `user_credentials.user_id` to `users`): deleting an app client or a user that
-- one of these rows names is refused, never cascaded.
-- `restore_openid_provider_live_postgres.rs` compares the result with a database
-- that stopped at 0008, catalog against catalog, so a drift in this file fails
-- there.
--
-- `cargo xtask migrations-current` requires the generator's own statements for
-- the five tables below verbatim (the blocks before the "Hand-written from
-- here" line), through ADDED_BY_LATER in `.xtask/src/migrations_current.rs`. It
-- strips them from the generator's output and, on the other side, strips the
-- same five tables from the frozen 0001_init (DROPPED_BY_LATER, which also
-- checks that 0009 drops every one), so the two strips cancel; it also refuses a
-- restoring migration that sorts before the one that drops. Squash all three into
-- a regenerated 0001_init and delete both entries together.
--
-- `sms-migrate` applies this file in one transaction with its bookkeeping row,
-- so a failure leaves nothing half-created. See down.sql for the rollback.

-- The emitter's own statements for the five tables, byte for byte: `cratestack
-- migrate diff` output, which `cargo xtask migrations-current` requires verbatim.

CREATE TABLE client_assertions (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    jti TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE oauth_clients (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    app_client_id TEXT,
    token_endpoint_auth_method TEXT NOT NULL,
    jwks TEXT,
    grant_types TEXT NOT NULL,
    scopes TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    require_pkce BOOLEAN NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    PRIMARY KEY (id)
);

CREATE TABLE oauth_signing_keys (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    private_key_pem TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    expires_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE TABLE sso_transactions (
    id TEXT NOT NULL,
    state_hash TEXT NOT NULL,
    nonce TEXT NOT NULL,
    code_verifier TEXT NOT NULL,
    console_request TEXT NOT NULL,
    is_retry BOOLEAN NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE user_credentials (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    PRIMARY KEY (id)
);

CREATE UNIQUE INDEX client_assertions_jti_key ON client_assertions (jti);

CREATE UNIQUE INDEX oauth_clients_client_id_key ON oauth_clients (client_id);

CREATE UNIQUE INDEX sso_transactions_state_hash_key ON sso_transactions (state_hash);

CREATE UNIQUE INDEX user_credentials_user_id_key ON user_credentials (user_id);

ALTER TABLE oauth_clients ADD CONSTRAINT oauth_clients_token_endpoint_auth_method_enum_check CHECK (token_endpoint_auth_method IN ('private_key_jwt', 'none'));

ALTER TABLE oauth_clients ADD CONSTRAINT oauth_clients_app_client_id_fkey FOREIGN KEY (app_client_id) REFERENCES app_clients (id);

ALTER TABLE user_credentials ADD CONSTRAINT user_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id);

-- Hand-written from here (the emitter writes none of it): what 0002_bootstrap
-- and 0005_sso_federation did for these tables.

-- Identifiers are `cs_cuid()`: `Cuid` is format-guarded on REST query filters as
-- [a-z0-9]{2,32}, so ids carry no prefix separator.
ALTER TABLE oauth_clients      ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE oauth_signing_keys ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE client_assertions  ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE user_credentials   ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sso_transactions   ALTER COLUMN id SET DEFAULT cs_cuid();

-- Timestamps mixin, and the one model without it (`sso_transactions` has only
-- `created_at`: nothing updates a row, so there is no `updated_at`).
ALTER TABLE oauth_clients ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE oauth_signing_keys ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE client_assertions ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE user_credentials ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sso_transactions ALTER COLUMN created_at SET DEFAULT now();

-- Nothing in the framework touches updated_at on write; `touch_updated_at()` is
-- 0002_bootstrap's, and is still there.
CREATE TRIGGER oauth_clients_touch BEFORE UPDATE ON oauth_clients
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER oauth_signing_keys_touch BEFORE UPDATE ON oauth_signing_keys
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER client_assertions_touch BEFORE UPDATE ON client_assertions
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER user_credentials_touch BEFORE UPDATE ON user_credentials
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- Multi-value columns are space-delimited TEXT with sentinel separators, because
-- scalar list fields panic the server macro. Empty is a single space.
ALTER TABLE oauth_clients     ALTER COLUMN scopes SET DEFAULT ' ',
                              ALTER COLUMN grant_types SET DEFAULT ' ',
                              ALTER COLUMN redirect_uris SET DEFAULT ' ';

-- The OP reads exactly one row at startup: the newest active signing key.
CREATE INDEX oauth_signing_keys_active_idx ON oauth_signing_keys (created_at DESC)
    WHERE active;

-- Reaping spent client assertions. A `jti` need only be remembered until its
-- own `exp`; after that the assertion is refused on `exp` regardless, so
-- keeping the row would only grow the table.
CREATE INDEX client_assertions_expiry_idx ON client_assertions (expires_at);

-- The reaper deletes SSO transactions past `expires_at`, oldest first.
CREATE INDEX sso_transactions_expiry_idx ON sso_transactions (expires_at);

-- private_key_jwt without a key is a client that can never authenticate;
-- `none` *with* a key is a public client someone believed was confidential.
--
-- "Without a key" has to mean an empty key set too, not just a NULL column:
-- `{"keys":[]}` is not null and is still keyless. The predicate is written to
-- be *total* -- it returns true or false for every JSON shape, never NULL --
-- because a NULL in a CHECK passes. `jsonb_typeof(...) = 'array'` alone is
-- NULL when `keys` is absent, and `jsonb_array_length` raises when `keys` is
-- an object, so neither is usable on its own. Verified against Postgres 16
-- for `{"keys":[{...}]}` (pass) and each of `{"keys":[]}`, `{}`,
-- `{"keys":null}`, `{"keys":{}}`, `{"keys":"abc"}`, `null`, `[]` (all fail).
--
-- What this does NOT do is validate the keys themselves -- `{"keys":[{}]}`
-- passes. Structural JWK validation needs to parse each key and belongs in
-- `provisionAppClient`, which parses them anyway. This constraint exists to
-- catch the registration that is empty or malformed at the top level, which
-- is the mistake people actually make.
--
-- A `jwks` that is not JSON at all fails on the `::jsonb` cast with a json
-- syntax error rather than a constraint violation. Still rejected, just with
-- a less obvious message.
ALTER TABLE oauth_clients ADD CONSTRAINT oauth_clients_auth_method_jwks_check
    CHECK ((token_endpoint_auth_method = 'private_key_jwt'
              AND COALESCE(jsonb_typeof(jwks::jsonb -> 'keys'), '') = 'array'
              AND jsonb_path_exists(jwks::jsonb, '$.keys[0]'))
        OR (token_endpoint_auth_method = 'none' AND jwks IS NULL));

-- A client that presents no credential at all has only PKCE standing between
-- it and anyone who can reach /token. `none` without require_pkce is an open
-- token endpoint, so the database refuses to store one.
ALTER TABLE oauth_clients ADD CONSTRAINT oauth_clients_public_requires_pkce_check
    CHECK (token_endpoint_auth_method <> 'none' OR require_pkce);
