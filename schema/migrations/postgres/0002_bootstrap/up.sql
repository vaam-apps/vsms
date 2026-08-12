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
ALTER TABLE oauth_signing_keys      ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE client_assertions       ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE messages                ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE message_parts           ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE delivery_receipts       ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE jobs                    ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE providers               ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE routes                  ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sender_ids              ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE sender_id_registrations ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE opt_outs                ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE consent_records         ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE operator_prefix_rules   ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE webhook_endpoints       ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE webhook_attempts        ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE users                   ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE roles                   ALTER COLUMN id SET DEFAULT cs_cuid();
ALTER TABLE user_credentials        ALTER COLUMN id SET DEFAULT cs_cuid();

-- Timestamps mixin, and other dbgenerated() columns.
ALTER TABLE apps ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE app_clients ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE oauth_clients ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE oauth_signing_keys ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE client_assertions ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sender_ids ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE sender_id_registrations ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE providers ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE routes ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE operator_prefix_rules ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE messages ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE message_parts ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE jobs ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE opt_outs ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE consent_records ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE webhook_endpoints ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE users ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE roles ALTER COLUMN created_at SET DEFAULT now(),
            ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE user_credentials ALTER COLUMN created_at SET DEFAULT now(),
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
CREATE TRIGGER oauth_signing_keys_touch BEFORE UPDATE ON oauth_signing_keys
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER client_assertions_touch BEFORE UPDATE ON client_assertions
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER sender_ids_touch BEFORE UPDATE ON sender_ids
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER sender_id_registrations_touch BEFORE UPDATE ON sender_id_registrations
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER providers_touch BEFORE UPDATE ON providers
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER routes_touch BEFORE UPDATE ON routes
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER operator_prefix_rules_touch BEFORE UPDATE ON operator_prefix_rules
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER messages_touch BEFORE UPDATE ON messages
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER message_parts_touch BEFORE UPDATE ON message_parts
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER jobs_touch BEFORE UPDATE ON jobs
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER opt_outs_touch BEFORE UPDATE ON opt_outs
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER consent_records_touch BEFORE UPDATE ON consent_records
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER webhook_endpoints_touch BEFORE UPDATE ON webhook_endpoints
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER users_touch BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER roles_touch BEFORE UPDATE ON roles
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
CREATE TRIGGER user_credentials_touch BEFORE UPDATE ON user_credentials
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
    from_state TEXT NOT NULL,
    to_state   TEXT NOT NULL,
    PRIMARY KEY (from_state, to_state),
    -- The native `message_state` enum type is gone as of cratestack-migrate
    -- 0.5.0 (see above); these CHECKs are what used to be free with the type.
    CONSTRAINT message_state_transitions_from_check
        CHECK (from_state IN ('accepted', 'queued', 'routed', 'submitted', 'delivered', 'uncertain', 'undelivered', 'failed', 'expired', 'rejected', 'cancelled')),
    CONSTRAINT message_state_transitions_to_check
        CHECK (to_state IN ('accepted', 'queued', 'routed', 'submitted', 'delivered', 'uncertain', 'undelivered', 'failed', 'expired', 'rejected', 'cancelled'))
);

INSERT INTO message_state_transitions (from_state, to_state) VALUES
    ('accepted','queued'),      ('accepted','rejected'),    ('accepted','cancelled'),
    ('accepted','expired'),
    ('queued','routed'),        ('queued','cancelled'),     ('queued','expired'),
    ('queued','failed'),
    ('routed','submitted'),     ('routed','queued'),        ('routed','failed'),
    ('routed','expired'),       ('routed','cancelled'),     ('routed','uncertain'),
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
    from_state TEXT NOT NULL,
    to_state   TEXT NOT NULL,
    PRIMARY KEY (from_state, to_state),
    CONSTRAINT job_state_transitions_from_check
        CHECK (from_state IN ('pending', 'running', 'succeeded', 'failed', 'dead', 'cancelled')),
    CONSTRAINT job_state_transitions_to_check
        CHECK (to_state IN ('pending', 'running', 'succeeded', 'failed', 'dead', 'cancelled'))
);

INSERT INTO job_state_transitions (from_state, to_state) VALUES
    ('pending','running'),  ('pending','cancelled'),
    ('running','succeeded'),('running','failed'),   ('running','pending'),
    ('failed','pending'),   ('failed','dead'),      ('failed','cancelled'),
    ('dead','pending');
-- succeeded, cancelled are terminal. `dead -> pending` (#56): the one
-- caller is `requeueJob` (crates/sms-api/src/procedures.rs) — an operator's
-- explicit "try this again" action from the admin Jobs screen, never
-- proposed by the automatic pipeline (`crates/sms-worker/src/jobs.rs`'s own
-- `apply_failure` only ever writes `failed -> {pending, dead}`, never reads
-- a `dead` row again). Same shape as `attempt_state_transitions`'
-- `dead -> pending` (#43) two sections below, added for the identical
-- reason: a `dead` job is otherwise a true dead end, and this is the one
-- sanctioned way back from it.

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

CREATE TABLE attempt_state_transitions (
    from_state TEXT NOT NULL,
    to_state   TEXT NOT NULL,
    PRIMARY KEY (from_state, to_state),
    CONSTRAINT attempt_state_transitions_from_check
        CHECK (from_state IN ('pending', 'delivering', 'succeeded', 'failed', 'dead')),
    CONSTRAINT attempt_state_transitions_to_check
        CHECK (to_state IN ('pending', 'delivering', 'succeeded', 'failed', 'dead'))
);

INSERT INTO attempt_state_transitions (from_state, to_state) VALUES
    ('pending','delivering'),    ('failed','delivering'),
    ('delivering','succeeded'),  ('delivering','failed'),  ('delivering','dead'),
    ('failed','pending'),        ('dead','pending');
-- succeeded is the only true terminal state. `delivering -> dead` covers
-- both reasons §8.5 stops retrying outright: `maxAttempts` exhausted, and
-- an immediate 410 Gone (which also deactivates the endpoint — hooks.rs,
-- not this trigger). `failed -> dead` does not exist: the exhausted-
-- attempts check happens once, at the delivering -> {failed | dead}
-- decision the hooks role's own write makes, not as a second hop through
-- failed.
--
-- `failed -> pending` and `dead -> pending` (#43): the replay edges.
-- `replayWebhookAttempt` (crates/sms-api/src/procedures.rs) is the only
-- caller of either — an operator's explicit "re-fire this after fixing the
-- receiving end" action, never proposed by the automatic pipeline. No
-- `succeeded -> pending` edge exists, on purpose: re-firing a webhook the
-- receiver already processed successfully is a materially more dangerous
-- operation than re-firing one that never got through, and this story
-- (#43) is about the latter. `delivering -> pending` also does not exist,
-- so a replay can never race a lease a worker currently holds — the
-- procedure's own read happens outside any lease the claim loop takes, and
-- `if_match(version)` on its write turns a race against a concurrent claim
-- into a `PreconditionFailed`, not a corrupted attempt.

CREATE OR REPLACE FUNCTION attempts_guard_transition() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.state IS NOT DISTINCT FROM OLD.state THEN
        RETURN NEW;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM attempt_state_transitions
        WHERE from_state = OLD.state AND to_state = NEW.state
    ) THEN
        RAISE EXCEPTION 'illegal webhook attempt transition % -> % on %', OLD.state, NEW.state, OLD.id
            USING ERRCODE = 'SM001';
    END IF;
    IF NEW.state = 'succeeded' AND NEW.delivered_at IS NULL THEN
        NEW.delivered_at := now();
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER attempts_state_guard
    BEFORE UPDATE ON webhook_attempts
    FOR EACH ROW EXECUTE FUNCTION attempts_guard_transition();

-- The dispatch claim path.
CREATE INDEX messages_dispatch_idx
    ON messages (priority DESC, created_at)
    WHERE state IN ('accepted','queued') AND lease_until IS NULL;

CREATE INDEX messages_lease_reclaim_idx
    ON messages (lease_until)
    WHERE lease_until IS NOT NULL AND state IN ('queued','routed','undelivered');

CREATE INDEX messages_app_created_idx   ON messages (app_id, created_at DESC);
CREATE INDEX messages_state_created_idx ON messages (state, created_at DESC);
CREATE INDEX messages_msisdn_hash_idx   ON messages (msisdn_hash, created_at DESC);

-- #67's purge_retention candidate query — a terminal message, not yet
-- purged, past its own createdAt cutoff. Partial and narrow, same style as
-- messages_dispatch_idx/messages_lease_reclaim_idx above, rather than
-- leaning on messages_state_created_idx alone: that index still has to
-- scan every non-purged row of five different states before this job's
-- own extra purged_at filter narrows it.
CREATE INDEX messages_purge_idx ON messages (created_at)
    WHERE purged_at IS NULL
      AND state IN ('delivered','failed','expired','rejected','cancelled');
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

-- The hooks role's crash-reclaim query (a stale `delivering` lease) — same
-- role `messages_lease_reclaim_idx`/`jobs_lease_reclaim_idx` play for their
-- own claim loops.
CREATE INDEX webhook_attempts_lease_reclaim_idx ON webhook_attempts (lease_until)
    WHERE state = 'delivering';

CREATE INDEX receipts_lookup_idx  ON delivery_receipts (provider_id, provider_message_ref);
CREATE INDEX receipts_message_idx ON delivery_receipts (message_id);
-- #67's purge_retention delete query — receipts age off their own
-- received_at, independent of their parent message's age (see §2.5).
CREATE INDEX receipts_received_at_idx ON delivery_receipts (received_at);
CREATE INDEX app_clients_app_idx  ON app_clients (app_id);
CREATE INDEX routes_match_idx     ON routes (enabled, priority DESC);

-- #72: `Procedures::ensure_consent_on_file`'s own lookup — no `@unique` on
-- `msisdnHash` here the way `OptOut.msisdnHash` has one, because a single
-- recipient can legitimately hold more than one ConsentRecord (different
-- scopes, or a re-consent over time; this model is append-only evidence,
-- not a single mutable flag — see its own schema.cstack comment).
CREATE INDEX consent_records_lookup_idx
    ON consent_records (app_id, msisdn_hash, scope);

-- The OP reads exactly one row at startup: the newest active signing key.
CREATE INDEX oauth_signing_keys_active_idx ON oauth_signing_keys (created_at DESC)
    WHERE active;

-- Reaping spent client assertions. A `jti` need only be remembered until its
-- own `exp`; after that the assertion is refused on `exp` regardless, so
-- keeping the row would only grow the table.
CREATE INDEX client_assertions_expiry_idx ON client_assertions (expires_at);

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

ALTER TABLE message_parts DROP CONSTRAINT message_parts_message_id_fkey;
ALTER TABLE message_parts ADD CONSTRAINT parts_message_fk
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE delivery_receipts DROP CONSTRAINT delivery_receipts_message_id_fkey;
ALTER TABLE delivery_receipts ADD CONSTRAINT receipts_message_fk
    FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE webhook_attempts DROP CONSTRAINT webhook_attempts_endpoint_id_fkey;
ALTER TABLE webhook_attempts ADD CONSTRAINT wha_endpoint_fk
    FOREIGN KEY (endpoint_id) REFERENCES webhook_endpoints(id) ON DELETE CASCADE;

ALTER TABLE operator_prefix_rules ADD CONSTRAINT operator_prefix_rules_prefix_format_check
    CHECK (prefix ~ '^[0-9]{1,4}$');

ALTER TABLE roles ADD CONSTRAINT roles_key_not_reserved_check
    CHECK (key NOT IN ('system', 'app'));

-- private_key_jwt without a key is a client that can never authenticate;
-- `none` *with* a key is a public client someone believed was confidential.
--
-- "Without a key" has to mean an empty key set too, not just a NULL column:
-- `{"keys":[]}` is not null and is still keyless. The predicate is written to
-- be *total* — it returns true or false for every JSON shape, never NULL —
-- because a NULL in a CHECK passes. `jsonb_typeof(...) = 'array'` alone is
-- NULL when `keys` is absent, and `jsonb_array_length` raises when `keys` is
-- an object, so neither is usable on its own. Verified against Postgres 16
-- for `{"keys":[{...}]}` (pass) and each of `{"keys":[]}`, `{}`,
-- `{"keys":null}`, `{"keys":{}}`, `{"keys":"abc"}`, `null`, `[]` (all fail).
--
-- What this does NOT do is validate the keys themselves — `{"keys":[{}]}`
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

INSERT INTO operator_prefix_rules (prefix, operator, confidence, notes, version) VALUES
    ('62',  'camtel', 'unverified', 'Camtel; unverified per architecture.md §3.4', 0),
    ('67',  'mtn',    'likely',     'MTN 67x per architecture.md §3.4', 0),
    ('68',  'unknown','contested',  'Contested between sources per architecture.md §3.4 — do not treat as reliable', 0),
    ('69',  'orange', 'likely',     'Orange 69x per architecture.md §3.4', 0),
    ('650', 'mtn',    'likely',     'MTN 650-654 per architecture.md §3.4', 0),
    ('651', 'mtn',    'likely',     'MTN 650-654 per architecture.md §3.4', 0),
    ('652', 'mtn',    'likely',     'MTN 650-654 per architecture.md §3.4', 0),
    ('653', 'mtn',    'likely',     'MTN 650-654 per architecture.md §3.4', 0),
    ('654', 'mtn',    'likely',     'MTN 650-654 per architecture.md §3.4', 0),
    ('655', 'orange', 'likely',     'Orange 655-659 per architecture.md §3.4', 0),
    ('656', 'orange', 'likely',     'Orange 655-659 per architecture.md §3.4', 0),
    ('657', 'orange', 'likely',     'Orange 655-659 per architecture.md §3.4', 0),
    ('658', 'orange', 'likely',     'Orange 655-659 per architecture.md §3.4', 0),
    ('659', 'orange', 'likely',     'Orange 655-659 per architecture.md §3.4', 0);
