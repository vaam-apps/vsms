-- 0002_bootstrap / up.sql
--
-- Everything cratestack-migrate does not emit: identifier and timestamp
-- defaults, the updated_at trigger, the two state machines, non-unique and
-- partial indexes, and foreign keys.
--
-- Generated from docs/architecture.md section 2.10 by ci/gen-bootstrap-sql.py.
-- Do not hand-edit: edit the document, regenerate, and commit both.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- `Cuid` is format-guarded on REST query filters as [a-z0-9]{2,32}, so ids must
-- carry NO prefix separator or `GET /messages?id=...` returns 400.
CREATE OR REPLACE FUNCTION cs_cuid() RETURNS TEXT AS $$
  SELECT 'c' || encode(gen_random_bytes(11), 'hex');   -- 23 chars, [a-z0-9]
$$ LANGUAGE SQL VOLATILE;

ALTER TABLE apps                    ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE app_clients             ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE oauth_clients           ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE messages                ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE message_parts           ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE delivery_receipts       ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE jobs                    ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE providers               ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE routes                  ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sender_ids              ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sender_id_registrations ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE opt_outs                ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE webhook_endpoints       ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE webhook_attempts        ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE users                   ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE roles                   ALTER COLUMN id SET DEFAULT cs_cuid();

-- Timestamps mixin, and other dbgenerated() columns.
ALTER TABLE apps ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE app_clients ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE oauth_clients ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sender_ids ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sender_id_registrations ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE providers ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE routes ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE messages ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE message_parts ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE jobs ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE opt_outs ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE webhook_endpoints ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE users ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE roles ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE delivery_receipts ALTER COLUMN received_at SET DEFAULT now();

-- Nothing in the framework touches updated_at on write, and remembering to set
-- it in every call site is the kind of thing that works until it doesn't.
-- clock_timestamp(), not now(): now() is the transaction timestamp, so two
-- updates to the same row inside one transaction would carry an identical
-- updated_at, and updated_at would equal created_at on a row created and
-- updated in the same transaction.
CREATE OR REPLACE FUNCTION touch_updated_at() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at := clock_timestamp();
    RETURN NEW;
END $$;

CREATE TRIGGER apps_touch BEFORE UPDATE ON apps
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER app_clients_touch BEFORE UPDATE ON app_clients
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER oauth_clients_touch BEFORE UPDATE ON oauth_clients
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER sender_ids_touch BEFORE UPDATE ON sender_ids
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER sender_id_registrations_touch BEFORE UPDATE ON sender_id_registrations
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER providers_touch BEFORE UPDATE ON providers
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER routes_touch BEFORE UPDATE ON routes
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER messages_touch BEFORE UPDATE ON messages
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER message_parts_touch BEFORE UPDATE ON message_parts
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER jobs_touch BEFORE UPDATE ON jobs
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER opt_outs_touch BEFORE UPDATE ON opt_outs
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER webhook_endpoints_touch BEFORE UPDATE ON webhook_endpoints
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER users_touch BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER roles_touch BEFORE UPDATE ON roles
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- Multi-value columns are space-delimited TEXT with sentinel separators (§2.2),
-- because scalar list fields panic the server macro. Empty is a single space.
ALTER TABLE app_clients       ALTER COLUMN scopes SET DEFAULT ' ';
ALTER TABLE oauth_clients     ALTER COLUMN scopes SET DEFAULT ' ',
                              ALTER COLUMN grant_types SET DEFAULT ' ',
                              ALTER COLUMN redirect_uris SET DEFAULT ' ';
ALTER TABLE apps              ALTER COLUMN ip_allowlist SET DEFAULT ' ';
ALTER TABLE roles             ALTER COLUMN permissions SET DEFAULT ' ';
ALTER TABLE webhook_endpoints ALTER COLUMN event_types SET DEFAULT ' ';

-- @version emits BIGINT NOT NULL with no default.
ALTER TABLE messages         ALTER COLUMN version SET DEFAULT 0;
ALTER TABLE webhook_attempts ALTER COLUMN version SET DEFAULT 0;
ALTER TABLE jobs             ALTER COLUMN version SET DEFAULT 0;

CREATE TABLE message_state_transitions (
    from_state message_state NOT NULL,
    to_state   message_state NOT NULL,
    PRIMARY KEY (from_state, to_state)
);

INSERT INTO message_state_transitions (from_state, to_state) VALUES
    ('accepted','queued'),      ('accepted','rejected'),    ('accepted','cancelled'),
    ('accepted','expired'),
    ('queued','routed'),        ('queued','cancelled'),     ('queued','expired'),
    ('queued','failed'),
    ('routed','submitted'),     ('routed','queued'),        ('routed','failed'),
    ('routed','expired'),       ('routed','cancelled'),
    ('submitted','delivered'),  ('submitted','uncertain'),  ('submitted','undelivered'),
    ('submitted','failed'),     ('submitted','expired'),
    ('uncertain','delivered'),  ('uncertain','failed'),     ('uncertain','expired'),
    ('undelivered','queued'),   ('undelivered','failed'),   ('undelivered','expired');
-- delivered, failed, expired, rejected, cancelled have NO outgoing rows.
-- Terminality is therefore data, not code: nothing leaves them.

CREATE OR REPLACE FUNCTION messages_guard_transition() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state IS NOT DISTINCT FROM OLD.state THEN
        RETURN NEW;
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM message_state_transitions
        WHERE from_state = OLD.state AND to_state = NEW.state
    ) THEN
        RAISE EXCEPTION
            'illegal message transition % -> % on %', OLD.state, NEW.state, OLD.id
            USING ERRCODE = 'SM001';
    END IF;

    IF NEW.state IN ('delivered','failed','expired','rejected','cancelled')
       AND NEW.finalized_at IS NULL THEN
        NEW.finalized_at := now();
    END IF;

    IF NEW.state = 'submitted' AND NEW.submitted_at IS NULL THEN
        NEW.submitted_at := now();
    END IF;

    RETURN NEW;
END $$;

CREATE TRIGGER messages_state_guard
    BEFORE UPDATE ON messages
    FOR EACH ROW EXECUTE FUNCTION messages_guard_transition();

CREATE TABLE job_state_transitions (
    from_state job_state NOT NULL,
    to_state   job_state NOT NULL,
    PRIMARY KEY (from_state, to_state)
);

INSERT INTO job_state_transitions (from_state, to_state) VALUES
    ('pending','running'),  ('pending','cancelled'),
    ('running','succeeded'),('running','failed'),   ('running','pending'),
    ('failed','pending'),   ('failed','dead'),      ('failed','cancelled');
-- succeeded, dead, cancelled are terminal.

CREATE OR REPLACE FUNCTION jobs_guard_transition() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state IS NOT DISTINCT FROM OLD.state THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM job_state_transitions
        WHERE from_state = OLD.state AND to_state = NEW.state
    ) THEN
        RAISE EXCEPTION 'illegal job transition % -> % on %', OLD.state, NEW.state, OLD.id
            USING ERRCODE = 'SM001';
    END IF;
    IF NEW.state = 'running'   AND NEW.started_at  IS NULL THEN NEW.started_at  := now(); END IF;
    IF NEW.state IN ('succeeded','dead','cancelled') AND NEW.finished_at IS NULL THEN
        NEW.finished_at := now();
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER jobs_state_guard
    BEFORE UPDATE ON jobs
    FOR EACH ROW EXECUTE FUNCTION jobs_guard_transition();

-- The dispatch claim path.
CREATE INDEX messages_dispatch_idx
    ON messages (priority DESC, created_at)
    WHERE state IN ('accepted','queued') AND lease_until IS NULL;

CREATE INDEX messages_lease_reclaim_idx
    ON messages (lease_until)
    WHERE lease_until IS NOT NULL AND state IN ('queued','routed');

CREATE INDEX messages_app_created_idx   ON messages (app_id, created_at DESC);
CREATE INDEX messages_state_created_idx ON messages (state, created_at DESC);
CREATE INDEX messages_msisdn_hash_idx   ON messages (msisdn_hash, created_at DESC);
CREATE INDEX messages_provider_ref_idx  ON messages (provider_id, provider_message_ref)
    WHERE provider_message_ref IS NOT NULL;
CREATE INDEX messages_provider_ref_alt_idx ON messages (provider_id, provider_message_ref_alt)
    WHERE provider_message_ref_alt IS NOT NULL;

CREATE UNIQUE INDEX messages_app_idem_key
    ON messages (app_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- The job claim path, plus dedupe on non-terminal jobs only.
CREATE INDEX jobs_claim_idx
    ON jobs (priority DESC, run_at)
    WHERE state = 'pending';

CREATE INDEX jobs_lease_reclaim_idx
    ON jobs (lease_until)
    WHERE state = 'running';

CREATE UNIQUE INDEX jobs_dedupe_idx
    ON jobs (kind, dedupe_key)
    WHERE dedupe_key IS NOT NULL AND state IN ('pending','running','failed');

-- THE webhook dedupe key. Not optional: see §8.3.
CREATE UNIQUE INDEX webhook_attempts_dedupe
    ON webhook_attempts (endpoint_id, aggregate_id, event_type);

CREATE INDEX webhook_due_idx ON webhook_attempts (next_attempt_at)
    WHERE state IN ('pending','failed');

CREATE INDEX receipts_lookup_idx  ON delivery_receipts (provider_id, provider_message_ref);
CREATE INDEX receipts_message_idx ON delivery_receipts (message_id);
CREATE INDEX app_clients_app_idx  ON app_clients (app_id);
CREATE INDEX routes_match_idx     ON routes (enabled, priority DESC);

-- The framework's own outbox. `ensure_event_outbox_table` creates this lazily
-- on the first emitting write, which is too late to index it here: applying
-- the migration to a fresh database fails with
--   ERROR: relation "cratestack_event_outbox" does not exist
-- Create it ourselves, with the framework's exact shape, so the index has
-- something to attach to. The framework's IF NOT EXISTS then no-ops.
CREATE TABLE IF NOT EXISTS cratestack_event_outbox (
    event_id UUID PRIMARY KEY,
    model TEXT NOT NULL,
    operation TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    delivered_at TIMESTAMPTZ,
    attempts BIGINT NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX cratestack_event_outbox_undelivered_idx
    ON cratestack_event_outbox (occurred_at, event_id)
    WHERE delivered_at IS NULL;

ALTER TABLE messages ADD CONSTRAINT messages_app_fk
    FOREIGN KEY (app_id) REFERENCES apps(id);
ALTER TABLE app_clients ADD CONSTRAINT app_clients_app_fk
    FOREIGN KEY (app_id) REFERENCES apps(id);
ALTER TABLE message_parts ADD CONSTRAINT parts_message_fk
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE delivery_receipts ADD CONSTRAINT receipts_message_fk
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE routes ADD CONSTRAINT routes_provider_fk
    FOREIGN KEY (provider_id) REFERENCES providers(id);
ALTER TABLE webhook_attempts ADD CONSTRAINT wha_endpoint_fk
    FOREIGN KEY (endpoint_id) REFERENCES webhook_endpoints(id) ON DELETE CASCADE;
ALTER TABLE users ADD CONSTRAINT users_role_fk
    FOREIGN KEY (role_key) REFERENCES roles(key);
