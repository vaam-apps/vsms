-- 0006_client_keys / up.sql
--
-- Signed transport (VS-2): the `ClientKey` model, an app client's registered
-- request-signing public key. 0001_init has already been applied to deployed
-- databases and `sms-migrate` records migrations by name, so a regenerated
-- 0001_init would never reach them - the new table ships as a forward
-- migration instead, the same way 0005_sso_federation does, and
-- `cargo xtask migrations-current` knows that (ADDED_BY_LATER in
-- .xtask/src/migrations_current.rs).
--
-- The first four statements are `cratestack migrate diff`'s own output for
-- this model, copied byte for byte (xtask checks the copy against a fresh
-- generator run, so it cannot drift from schemas/vsms.cstack). The rest is
-- what the emitter cannot write (docs/architecture.md section 2.0 - no column
-- defaults, no non-unique indexes, no CHECKs for @length) and what
-- 0002_bootstrap does for every older table.

CREATE TABLE client_keys (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    app_client_id TEXT NOT NULL,
    alg TEXT NOT NULL,
    public_key BYTEA NOT NULL,
    thumbprint TEXT NOT NULL,
    label TEXT,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE UNIQUE INDEX client_keys_thumbprint_key ON client_keys (thumbprint);

ALTER TABLE client_keys ADD CONSTRAINT client_keys_alg_enum_check CHECK (alg IN ('ed25519'));

ALTER TABLE client_keys ADD CONSTRAINT client_keys_app_client_id_fkey FOREIGN KEY (app_client_id) REFERENCES app_clients (id);

-- Hand-written from here (the emitter writes none of it).

ALTER TABLE client_keys ALTER COLUMN id SET DEFAULT cs_cuid();

ALTER TABLE client_keys ALTER COLUMN created_at SET DEFAULT now(),
                        ALTER COLUMN updated_at SET DEFAULT now();

-- `touch_updated_at` is 0002_bootstrap's trigger function. Unlike the SSO
-- tables, a key row is updated (revocation stamps `revoked_at`), so
-- `updated_at` has to move with it.
CREATE TRIGGER client_keys_touch BEFORE UPDATE ON client_keys
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- `@length` on these two is validated by the generated input types and the
-- procedure, not by the database (only `@db_enforce` would add a CHECK, and
-- this repo writes its CHECKs by hand). A key that is not 32 bytes cannot be
-- an Ed25519 point, and a thumbprint that is not 64 lowercase hex characters
-- cannot be the SHA-256 the procedure computes; a row written around the
-- procedure by a future bug is refused here rather than failing a signature
-- check later.
ALTER TABLE client_keys ADD CONSTRAINT client_keys_public_key_len_check
    CHECK (octet_length(public_key) = 32);
ALTER TABLE client_keys ADD CONSTRAINT client_keys_thumbprint_format_check
    CHECK (thumbprint ~ '^[0-9a-f]{64}$');

-- Not covered by the FK itself in Postgres; "every key of this client" is
-- both the console's list and VS-3's resolver query.
CREATE INDEX client_keys_app_client_idx ON client_keys (app_client_id);
