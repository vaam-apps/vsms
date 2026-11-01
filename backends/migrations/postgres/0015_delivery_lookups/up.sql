-- 0015_delivery_lookups / up.sql
--
-- The body-free delivery-status lookup (`lookupDeliveryStatus`, gated on
-- `delivery:read`): the `DeliveryLookup` model, one row per answered lookup. It
-- records who asked, by which kind of key, and how many messages came back. It
-- also is the procedure's budget: a caller's number lookups in the last hour are
-- counted from it (`MSISDN_LOOKUPS_PER_HOUR`). 0001_init has already been applied
-- to deployed databases and `sms-migrate` records migrations by name, so the new
-- table ships as a forward migration, like 0005 to 0008, and
-- `cargo xtask migrations-current` knows that (ADDED_BY_LATER in
-- .xtask/src/migrations_current.rs).
--
-- The first two statements are `cratestack migrate diff`'s own output for this
-- model, copied byte for byte (xtask checks the copy against a fresh generator
-- run, so it cannot drift from schemas/vsms.cstack). The rest is what the
-- emitter cannot write (docs/architecture.md section 2.0 - no column defaults,
-- no non-unique indexes, no CHECKs for @length).
--
-- NEW TABLE ONLY. No existing table, column or policy changes, and no role
-- changes: `delivery:read` was seeded by 0002_bootstrap for `auditor` and
-- `support` and 0013/0014 left it there. Nothing is backfilled.
--
-- `keyValue` is never the thing a person typed: a message id, or a keyed hash of
-- the number or of the client reference. See the model's comment in
-- schemas/vsms.cstack.

CREATE TABLE delivery_lookups (
    id TEXT NOT NULL,
    actor_sub TEXT NOT NULL,
    actor_kind TEXT NOT NULL,
    actor_role TEXT NOT NULL,
    actor_app_id TEXT,
    key_kind TEXT NOT NULL,
    key_value TEXT NOT NULL,
    app_id_filter TEXT,
    result_count BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_key_kind_enum_check CHECK (key_kind IN ('message_id', 'client_ref', 'msisdn'));

-- Hand-written from here (the emitter writes none of it).

ALTER TABLE delivery_lookups ALTER COLUMN id SET DEFAULT cs_cuid();

-- Insert-only (no `update` action exists), so there is no `updated_at` and no
-- `touch_updated_at` trigger.
ALTER TABLE delivery_lookups ALTER COLUMN created_at SET DEFAULT now();

-- `@length` is validated by the generated input type, not by the database; a row
-- written around the procedure by a future bug is refused here.
ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_actor_sub_length_check
    CHECK (char_length(actor_sub) BETWEEN 1 AND 255);
ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_actor_kind_length_check
    CHECK (char_length(actor_kind) BETWEEN 1 AND 16);
ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_actor_role_length_check
    CHECK (char_length(actor_role) BETWEEN 1 AND 64);
ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_key_value_length_check
    CHECK (char_length(key_value) BETWEEN 1 AND 255);
ALTER TABLE delivery_lookups ADD CONSTRAINT delivery_lookups_result_count_check
    CHECK (result_count >= 0);

-- The budget's query: "this caller's number lookups since an hour ago" is an
-- equality on the caller and a range on time, newest first. Also "what has this
-- person looked up" for an auditor reading the table.
CREATE INDEX delivery_lookups_actor_created_idx ON delivery_lookups (actor_sub, created_at DESC);
