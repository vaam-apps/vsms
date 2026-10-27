-- 0010_sealed_webhooks / up.sql
--
-- VS-8: a webhook delivery is a sealed request (a COSE_Sign1 envelope signed
-- with vsms's envelope signing key), not an HMAC under a per-endpoint shared
-- secret. This migration reshapes `webhook_endpoints` to match:
--
--   audience            NEW, NOT NULL, UNIQUE: the receiver's identity, the audience
--                       a delivery's seal binds (cratestack ADR 0006 section 4)
--   secret              DROPPED: the HMAC signing secret
--   prev_secret         DROPPED: the rotation-overlap secret
--   secret_rotated_at   DROPPED: when `secret` was last rotated
--
-- Why `audience` is UNIQUE, and never `vsms`. One gateway key signs every app's
-- deliveries, so the audience is what tells one receiver's seals from
-- another's. Were it free, whoever holds `webhook:manage` could create an
-- endpoint that names a victim's audience and points at a URL of their own,
-- collect seals the victim's receiver would open, and replay them inside the
-- skew window. The unique index closes the name (an audience is taken once,
-- first come), and the delivery envelope's `appId` and `endpointId` close the
-- rest: a receiver compares them with its own (`sms-webhook`'s `lib.md`, "What
-- a receiver configures"). `vsms` is the name the gateway's own routes answer
-- to, so a receiver that used it in both directions would give up the
-- protection against reflecting a request back; the CHECK refuses it. The
-- generator emits the unique index and the length CHECK (`@unique`,
-- `@length` with `@db_enforce`), and `cargo xtask migrations-current` requires
-- both verbatim below; it cannot emit the `vsms` CHECK (`@db_enforce` does
-- nothing for the `@regex` that refuses it at the API), so that one is
-- written here, and a live test (sealed_webhooks_migration_live_postgres.rs)
-- proves it.
--
-- Backfill of `audience` for rows that already exist: the URL's origin
-- (`scheme://host[:port]`, lower-cased, any `user:password@` removed so a
-- credential is never copied into a column the console shows). There is no
-- default that could stand for "any receiver" - a seal binds exactly one
-- recipient - so an existing row gets the most likely name of its receiver
-- instead, which is also what the ADR's own example uses. Two rows with one
-- origin (two apps on one host, or one app with two paths) cannot both have it:
-- the oldest keeps the bare origin and every later one gets `#<endpoint id>`
-- appended, so the unique index can be built. A URL with no `scheme://host`
-- (the `@uri` validator makes that impossible for a row the API wrote) gets
-- `endpoint-<endpoint id>`, never a copy of the URL. The result is capped at
-- the column's 255 characters. It is a starting value, not a guess to trust: a
-- receiver that opens deliveries with a cratestack `EnvelopeLayer` is
-- configured with its own audience, and an operator sets that on the endpoint
-- (the console's endpoint screen, or `PATCH /webhook_endpoints/{id}`) when the
-- receiver is moved onto the layer. Until then its deliveries are refused with
-- a 401 and retried on the usual backoff; nothing is lost, because a refused
-- attempt stays `failed`/`dead` and can be replayed once the receiver is fixed.
--
-- `0001_init` has already been applied to deployed databases and `sms-migrate`
-- records migrations by name, so it cannot stop creating the three secret
-- columns; this forward migration removes them, and `cargo xtask
-- migrations-current` knows that (COLUMNS_CHANGED_BY_LATER in
-- .xtask/src/migrations_current.rs): it takes these four columns out of both
-- sides before comparing `0001_init` with the generator, and checks that this
-- file adds and drops exactly the ones it lists.
--
-- The secrets are not gone from everywhere. `webhook_endpoints` is an `@@audit`
-- model, and `cratestack_audit` holds a snapshot of the whole row at every
-- create and update. `secret` and `prevSecret` were never `@sensitive`, so those
-- snapshots carry every HMAC secret this table ever held in plaintext, and the
-- anchor chain `anchor_audit` writes over them exists precisely so that history
-- cannot be rewritten: this migration does not scrub them and no later one
-- should try. Treat every secret as readable by anyone who could read the audit
-- table, or a backup taken before 0010, and say so to each receiver: at the
-- cutover it must delete its HMAC verification and the secret it stored, not
-- merely stop sending it. (A receiver that keeps verifying an HMAC alongside the
-- seal has kept a way for anyone who can read that history to forge a
-- delivery.)
--
-- Irreversible by intent: the secrets must not come back from a `down`. See
-- down.sql.

ALTER TABLE webhook_endpoints ADD COLUMN audience TEXT;

WITH origin AS (
    SELECT
        id,
        created_at,
        CASE
            WHEN m.parts IS NULL THEN NULL
            ELSE left(lower(m.parts[1] || '://' || m.parts[2]), 255)
        END AS base
    FROM webhook_endpoints
    LEFT JOIN LATERAL (
        SELECT regexp_match(url, '^([A-Za-z][A-Za-z0-9+.-]*)://(?:[^/?#]*@)?([^/?#]+)') AS parts
    ) AS m ON true
),
ranked AS (
    SELECT
        id,
        base,
        row_number() OVER (PARTITION BY base ORDER BY created_at, id) AS rank
    FROM origin
)
UPDATE webhook_endpoints AS e
SET audience = CASE
    WHEN r.base IS NULL THEN 'endpoint-' || e.id
    WHEN r.rank = 1 THEN r.base
    ELSE left(r.base, 255 - length(e.id) - 1) || '#' || e.id
END
FROM ranked AS r
WHERE e.id = r.id;

ALTER TABLE webhook_endpoints ALTER COLUMN audience SET NOT NULL;

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_length_check CHECK (length(audience) BETWEEN 1 AND 255);

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_not_vsms_check CHECK (audience <> 'vsms');

CREATE UNIQUE INDEX webhook_endpoints_audience_key ON webhook_endpoints (audience);

ALTER TABLE webhook_endpoints DROP COLUMN secret;

ALTER TABLE webhook_endpoints DROP COLUMN prev_secret;

ALTER TABLE webhook_endpoints DROP COLUMN secret_rotated_at;
