-- WARNING: this migration contains blocking operations.
-- A required column was added without a default. The migration
-- will fail on a non-empty table unless an `up.pre.sql` backfills
-- the affected columns before this statement runs.

-- NOTE: the following column(s) use `@default(dbgenerated())`, a
-- marker meaning the value is expected to come from a real
-- Postgres-level default set some other way (hand-authored SQL, a
-- trigger, GENERATED ... AS IDENTITY, etc). cratestack does not
-- emit a DEFAULT clause for it. If no such default exists,
-- INSERTs that omit the column will fail with a NOT NULL violation:
--   - app_clients.created_at
--   - app_clients.updated_at
--   - app_clients.id
--   - apps.created_at
--   - apps.updated_at
--   - apps.id
--   - delivery_receipts.id
--   - delivery_receipts.received_at
--   - jobs.created_at
--   - jobs.updated_at
--   - jobs.id
--   - message_parts.created_at
--   - message_parts.updated_at
--   - message_parts.id
--   - messages.created_at
--   - messages.updated_at
--   - messages.id
--   - oauth_clients.created_at
--   - oauth_clients.updated_at
--   - oauth_clients.id
--   - operator_prefixs.created_at
--   - operator_prefixs.updated_at
--   - operator_prefixs.id
--   - opt_outs.created_at
--   - opt_outs.updated_at
--   - opt_outs.id
--   - providers.created_at
--   - providers.updated_at
--   - providers.id
--   - roles.created_at
--   - roles.updated_at
--   - roles.id
--   - routes.created_at
--   - routes.updated_at
--   - routes.id
--   - sender_id_registrations.created_at
--   - sender_id_registrations.updated_at
--   - sender_id_registrations.id
--   - sender_ids.created_at
--   - sender_ids.updated_at
--   - sender_ids.id
--   - users.created_at
--   - users.updated_at
--   - users.id
--   - webhook_attempts.id
--   - webhook_endpoints.created_at
--   - webhook_endpoints.updated_at
--   - webhook_endpoints.id

CREATE TABLE app_clients (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    label TEXT NOT NULL,
    scopes TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    last_used_at TIMESTAMPTZ,
    retired_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE TABLE apps (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    slug TEXT NOT NULL,
    description TEXT,
    default_sender_id_id TEXT,
    monthly_quota BIGINT NOT NULL,
    ip_allowlist TEXT NOT NULL,
    transliterate_to_gsm7 BOOLEAN NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE TABLE delivery_receipts (
    id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    provider_message_ref TEXT NOT NULL,
    outcome TEXT NOT NULL,
    raw_status TEXT NOT NULL,
    error_code TEXT,
    network_code TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL,
    occurred_at TIMESTAMPTZ,
    raw_payload TEXT NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE jobs (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    dedupe_key TEXT,
    payload TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    priority BIGINT NOT NULL,
    run_at TIMESTAMPTZ NOT NULL,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    attempts BIGINT NOT NULL DEFAULT 0,
    max_attempts BIGINT NOT NULL,
    last_error TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    version BIGINT NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE message_parts (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    part_index BIGINT NOT NULL,
    udh_ref BIGINT,
    provider_part_ref TEXT,
    state TEXT NOT NULL DEFAULT 'queued',
    submitted_at TIMESTAMPTZ,
    delivered_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE TABLE messages (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    client_ref TEXT,
    idempotency_key TEXT,
    msisdn TEXT NOT NULL,
    msisdn_hash TEXT NOT NULL,
    operator TEXT NOT NULL,
    sender_id_value TEXT NOT NULL,
    class TEXT NOT NULL,
    priority BIGINT NOT NULL,
    body TEXT,
    body_hash TEXT NOT NULL,
    body_length BIGINT NOT NULL,
    encoding TEXT NOT NULL,
    segments BIGINT NOT NULL,
    state TEXT NOT NULL DEFAULT 'accepted',
    state_reason TEXT,
    route_id TEXT,
    provider_id TEXT,
    provider_message_ref TEXT,
    provider_message_ref_alt TEXT,
    attempts BIGINT NOT NULL DEFAULT 0,
    max_attempts BIGINT NOT NULL,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    scheduled_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    submitted_at TIMESTAMPTZ,
    finalized_at TIMESTAMPTZ,
    cost_xaf NUMERIC NOT NULL DEFAULT 0,
    version BIGINT NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE oauth_clients (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    app_client_id TEXT,
    secret_hash TEXT NOT NULL,
    grant_types TEXT NOT NULL,
    scopes TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    require_pkce BOOLEAN NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    PRIMARY KEY (id)
);

CREATE TABLE operator_prefixs (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    prefix TEXT NOT NULL,
    operator TEXT NOT NULL,
    source TEXT NOT NULL,
    confidence BIGINT NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE opt_outs (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    msisdn_hash TEXT NOT NULL,
    msisdn TEXT NOT NULL,
    source TEXT NOT NULL,
    scope TEXT NOT NULL,
    reason TEXT,
    opted_out_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE providers (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'disabled',
    config TEXT NOT NULL,
    credential_ref TEXT NOT NULL,
    max_tps DOUBLE PRECISION NOT NULL,
    max_daily_submissions BIGINT NOT NULL,
    supports_dlr BOOLEAN NOT NULL,
    supports_alpha_sender BOOLEAN NOT NULL,
    supports_ucs2 BOOLEAN NOT NULL,
    supports_concat BOOLEAN NOT NULL,
    cost_per_segment_xaf NUMERIC NOT NULL,
    health_checked_at TIMESTAMPTZ,
    healthy BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (id)
);

CREATE TABLE roles (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    key TEXT NOT NULL,
    label TEXT NOT NULL,
    description TEXT,
    builtin BOOLEAN NOT NULL DEFAULT false,
    permissions TEXT NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE routes (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    priority BIGINT NOT NULL,
    weight BIGINT NOT NULL,
    enabled BOOLEAN NOT NULL,
    match_operator TEXT,
    match_class TEXT,
    match_app_id TEXT,
    match_prefix TEXT,
    provider_id TEXT NOT NULL,
    failover_route_id TEXT,
    PRIMARY KEY (id)
);

CREATE TABLE sender_id_registrations (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    sender_id_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    status TEXT NOT NULL,
    submitted_at TIMESTAMPTZ,
    approved_at TIMESTAMPTZ,
    reference TEXT,
    rejection_reason TEXT,
    PRIMARY KEY (id)
);

CREATE TABLE sender_ids (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    value TEXT NOT NULL,
    kind TEXT NOT NULL,
    notes TEXT,
    active BOOLEAN NOT NULL DEFAULT false,
    PRIMARY KEY (id)
);

CREATE TABLE users (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    subject TEXT NOT NULL,
    email TEXT NOT NULL,
    display_name TEXT NOT NULL,
    role_key TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    last_login_at TIMESTAMPTZ,
    mfa_enrolled BOOLEAN NOT NULL DEFAULT false,
    deleted_at TIMESTAMPTZ,
    PRIMARY KEY (id)
);

CREATE TABLE webhook_attempts (
    id TEXT NOT NULL,
    endpoint_id TEXT NOT NULL,
    source_event_id UUID NOT NULL,
    aggregate_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    attempts BIGINT NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    next_attempt_at TIMESTAMPTZ,
    last_status_code BIGINT,
    last_error TEXT,
    last_attempt_at TIMESTAMPTZ,
    delivered_at TIMESTAMPTZ,
    version BIGINT NOT NULL,
    PRIMARY KEY (id)
);

CREATE TABLE webhook_endpoints (
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    url TEXT NOT NULL,
    event_types TEXT NOT NULL,
    secret TEXT NOT NULL,
    prev_secret TEXT,
    secret_rotated_at TIMESTAMPTZ,
    mask_recipient BOOLEAN NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    max_attempts BIGINT NOT NULL,
    circuit_open_until TIMESTAMPTZ,
    consecutive_failures BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (id)
);

CREATE UNIQUE INDEX app_clients_client_id_key ON app_clients (client_id);

CREATE UNIQUE INDEX apps_slug_key ON apps (slug);

CREATE UNIQUE INDEX oauth_clients_client_id_key ON oauth_clients (client_id);

CREATE UNIQUE INDEX operator_prefixs_prefix_key ON operator_prefixs (prefix);

CREATE UNIQUE INDEX opt_outs_msisdn_hash_key ON opt_outs (msisdn_hash);

CREATE UNIQUE INDEX providers_key_key ON providers (key);

CREATE UNIQUE INDEX roles_key_key ON roles (key);

CREATE UNIQUE INDEX sender_ids_value_key ON sender_ids (value);

CREATE UNIQUE INDEX users_subject_key ON users (subject);

CREATE UNIQUE INDEX users_email_key ON users (email);

ALTER TABLE delivery_receipts ADD CONSTRAINT delivery_receipts_outcome_enum_check CHECK (outcome IN ('delivered', 'uncertain', 'failed', 'expired', 'rejected', 'unknown'));

ALTER TABLE delivery_receipts ADD CONSTRAINT delivery_receipts_network_code_enum_check CHECK (network_code IN ('mtn', 'orange', 'camtel', 'nexttel', 'unknown'));

ALTER TABLE jobs ADD CONSTRAINT jobs_state_enum_check CHECK (state IN ('pending', 'running', 'succeeded', 'failed', 'dead', 'cancelled'));

ALTER TABLE message_parts ADD CONSTRAINT message_parts_state_enum_check CHECK (state IN ('accepted', 'queued', 'routed', 'submitted', 'delivered', 'uncertain', 'undelivered', 'failed', 'expired', 'rejected', 'cancelled'));

ALTER TABLE messages ADD CONSTRAINT messages_operator_enum_check CHECK (operator IN ('mtn', 'orange', 'camtel', 'nexttel', 'unknown'));

ALTER TABLE messages ADD CONSTRAINT messages_class_enum_check CHECK (class IN ('otp', 'transactional', 'notification', 'marketing'));

ALTER TABLE messages ADD CONSTRAINT messages_encoding_enum_check CHECK (encoding IN ('gsm7', 'ucs2'));

ALTER TABLE messages ADD CONSTRAINT messages_state_enum_check CHECK (state IN ('accepted', 'queued', 'routed', 'submitted', 'delivered', 'uncertain', 'undelivered', 'failed', 'expired', 'rejected', 'cancelled'));

ALTER TABLE operator_prefixs ADD CONSTRAINT operator_prefixs_operator_enum_check CHECK (operator IN ('mtn', 'orange', 'camtel', 'nexttel', 'unknown'));

ALTER TABLE operator_prefixs ADD CONSTRAINT operator_prefixs_source_enum_check CHECK (source IN ('seed', 'dlr', 'manual'));

ALTER TABLE opt_outs ADD CONSTRAINT opt_outs_source_enum_check CHECK (source IN ('inbound_stop', 'admin', 'import', 'operator'));

ALTER TABLE providers ADD CONSTRAINT providers_kind_enum_check CHECK (kind IN ('orange_cm_http', 'mtn_http', 'aggregator_http', 'smpp'));

ALTER TABLE providers ADD CONSTRAINT providers_state_enum_check CHECK (state IN ('active', 'degraded', 'disabled', 'draining'));

ALTER TABLE routes ADD CONSTRAINT routes_match_operator_enum_check CHECK (match_operator IN ('mtn', 'orange', 'camtel', 'nexttel', 'unknown'));

ALTER TABLE routes ADD CONSTRAINT routes_match_class_enum_check CHECK (match_class IN ('otp', 'transactional', 'notification', 'marketing'));

ALTER TABLE sender_ids ADD CONSTRAINT sender_ids_value_length_check CHECK (length(value) BETWEEN 3 AND 11);

ALTER TABLE webhook_attempts ADD CONSTRAINT webhook_attempts_state_enum_check CHECK (state IN ('pending', 'delivering', 'succeeded', 'failed', 'dead'));

