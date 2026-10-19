-- 0005_sso_federation / up.sql
--
-- OIDC single sign-on (Keycloak): the `FederatedLink` and `SsoTransaction`
-- models. 0001_init has already been applied to deployed databases and
-- `sms-migrate` records migrations by name, so a regenerated 0001_init would
-- never reach them - the new tables ship as a forward migration instead, the
-- same way 0004_import_to_imported does, and `cargo xtask migrations-current`
-- knows that (ADDED_BY_LATER in .xtask/src/migrations_current.rs).
--
-- The first four statements are `cratestack migrate diff`'s own output for
-- these two models, copied byte for byte (xtask checks the copy against a
-- fresh generator run, so it cannot drift from schemas/vsms.cstack). The
-- rest is what the emitter cannot write (docs/architecture.md section 2.0 -
-- no column defaults, no composite uniques, no non-unique indexes) and what
-- 0002_bootstrap does for every older table.

CREATE TABLE federated_links (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    user_id TEXT NOT NULL,
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

CREATE UNIQUE INDEX sso_transactions_state_hash_key ON sso_transactions (state_hash);

ALTER TABLE federated_links ADD CONSTRAINT federated_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id);

-- Hand-written from here (the emitter writes none of it).

ALTER TABLE federated_links  ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sso_transactions ALTER COLUMN id SET DEFAULT cs_cuid();

ALTER TABLE federated_links ALTER COLUMN created_at SET DEFAULT now(),
                            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sso_transactions ALTER COLUMN created_at SET DEFAULT now();

-- The identity key. An upstream `sub` is unique only within its issuer, and a
-- link is looked up by exactly this pair on every federated sign-in. As a
-- UNIQUE index it is also what makes two concurrent first sign-ins for one
-- identity unable to both create a link (the loser gets SQLSTATE 23505 on
-- this constraint name and re-reads). Never `email`: see FederatedLink's
-- schema comment.
CREATE UNIQUE INDEX federated_links_issuer_subject_key ON federated_links (issuer, subject);

-- Not covered by the FK itself in Postgres; "which link belongs to this
-- user" is the support question when a sign-in misbehaves.
CREATE INDEX federated_links_user_id_idx ON federated_links (user_id);

-- The reaper (`sso.rs::spawn_transaction_reaper`) deletes rows past
-- `expires_at`, oldest first.
CREATE INDEX sso_transactions_expiry_idx ON sso_transactions (expires_at);
