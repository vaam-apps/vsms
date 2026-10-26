-- 0007_console_keys / up.sql
--
-- Signed transport (VS-5a): the `ConsoleKey` model, a human's registered
-- console request-signing public key. 0001_init has already been applied to
-- deployed databases and `sms-migrate` records migrations by name, so a
-- regenerated 0001_init would never reach them - the new table ships as a
-- forward migration instead, the same way 0005_sso_federation and
-- 0006_client_keys do, and `cargo xtask migrations-current` knows that
-- (ADDED_BY_LATER in .xtask/src/migrations_current.rs).
--
-- The first four statements are `cratestack migrate diff`'s own output for
-- this model, copied byte for byte (xtask checks the copy against a fresh
-- generator run, so it cannot drift from schemas/vsms.cstack). The rest is
-- what the emitter cannot write (docs/architecture.md section 2.0 - no column
-- defaults, no non-unique indexes, no CHECKs for @length) and what
-- 0002_bootstrap does for every older table.

CREATE TABLE console_keys (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    alg TEXT NOT NULL,
    public_key BYTEA NOT NULL,
    thumbprint TEXT NOT NULL,
    label TEXT,
    paired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE UNIQUE INDEX console_keys_thumbprint_key ON console_keys (thumbprint);

ALTER TABLE console_keys ADD CONSTRAINT console_keys_alg_enum_check CHECK (alg IN ('esp256'));

ALTER TABLE console_keys ADD CONSTRAINT console_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id);

-- Hand-written from here (the emitter writes none of it).

ALTER TABLE console_keys ALTER COLUMN id SET DEFAULT cs_cuid();

ALTER TABLE console_keys ALTER COLUMN created_at SET DEFAULT now(),
                         ALTER COLUMN updated_at SET DEFAULT now();

-- `touch_updated_at` is 0002_bootstrap's trigger function. A key row is
-- updated (revocation stamps `revoked_at`), so `updated_at` has to move with
-- it.
CREATE TRIGGER console_keys_touch BEFORE UPDATE ON console_keys
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- `@length` is validated by the generated input types and the procedure, not
-- by the database. A public key that is not 65 bytes starting with the SEC1
-- uncompressed tag 0x04 cannot be the P-256 point the procedure stores, and a
-- thumbprint that is not 64 lowercase hex characters cannot be the SHA-256 it
-- computes; a row written around the procedure by a future bug is refused
-- here rather than failing a signature check later. A key that expires
-- before it was paired is a bug in whoever computed the lifetime.
ALTER TABLE console_keys ADD CONSTRAINT console_keys_public_key_sec1_check
    CHECK (octet_length(public_key) = 65 AND get_byte(public_key, 0) = 4);
ALTER TABLE console_keys ADD CONSTRAINT console_keys_thumbprint_format_check
    CHECK (thumbprint ~ '^[0-9a-f]{64}$');
ALTER TABLE console_keys ADD CONSTRAINT console_keys_lifetime_check
    CHECK (expires_at > paired_at);

-- Not covered by the FK itself in Postgres; "every key of this user" is the
-- console's own list and the owner/admin view.
CREATE INDEX console_keys_user_id_idx ON console_keys (user_id);
