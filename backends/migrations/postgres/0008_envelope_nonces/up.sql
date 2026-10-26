-- 0008_envelope_nonces / up.sql
--
-- Signed transport (VS-3): the `EnvelopeNonce` model, the replay cache of the
-- envelope layer - one row per `(kid, cti)` accepted, so a captured sealed
-- request is refused the second time it arrives inside its freshness window.
-- cratestack ships no Postgres nonce store, and this deployment has no Redis,
-- so the table is the store every replica shares. 0001_init has already been
-- applied to deployed databases and `sms-migrate` records migrations by name,
-- so the new table ships as a forward migration, like 0005 to 0007, and
-- `cargo xtask migrations-current` knows that (ADDED_BY_LATER in
-- .xtask/src/migrations_current.rs).
--
-- The first two statements are `cratestack migrate diff`'s own output for this
-- model, copied byte for byte (xtask checks the copy against a fresh generator
-- run, so it cannot drift from schemas/vsms.cstack). The rest is what the
-- emitter cannot write (docs/architecture.md section 2.0 - no column defaults,
-- no non-unique indexes, no CHECKs for @length).

CREATE TABLE envelope_nonces (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    nonce TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

CREATE UNIQUE INDEX envelope_nonces_nonce_key ON envelope_nonces (nonce);

-- Hand-written from here (the emitter writes none of it).

ALTER TABLE envelope_nonces ALTER COLUMN id SET DEFAULT cs_cuid();

ALTER TABLE envelope_nonces ALTER COLUMN created_at SET DEFAULT now(),
                            ALTER COLUMN updated_at SET DEFAULT now();

-- `@length(min: 1, max: 255)` is validated by the generated input type, not
-- by the database; a nonce that long is a bug in whoever built the key.
ALTER TABLE envelope_nonces ADD CONSTRAINT envelope_nonces_nonce_length_check
    CHECK (char_length(nonce) BETWEEN 1 AND 255);

-- Sweeping the expired. A nonce need only be remembered until its own
-- `expires_at` (`iat + 2 * skew + 1`); after that the request is refused on
-- `iat` freshness regardless, so keeping the row would only grow the table.
-- The worker's `sweep_envelope_nonces` deletes by this column.
CREATE INDEX envelope_nonces_expiry_idx ON envelope_nonces (expires_at);
