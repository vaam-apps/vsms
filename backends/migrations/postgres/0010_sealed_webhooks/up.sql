-- 0010_sealed_webhooks / up.sql
--
-- VS-8: a webhook delivery is a sealed request (a COSE_Sign1 envelope signed
-- with vsms's envelope signing key), not an HMAC under a per-endpoint shared
-- secret. This migration reshapes `webhook_endpoints` to match:
--
--   audience            NEW, NOT NULL: the receiver's identity, the audience a
--                       delivery's seal binds (cratestack ADR 0006 section 4)
--   secret              DROPPED: the HMAC signing secret
--   prev_secret         DROPPED: the rotation-overlap secret
--   secret_rotated_at   DROPPED: when `secret` was last rotated
--
-- Backfill of `audience` for rows that already exist: the URL's origin
-- (`scheme://host[:port]`). There is no default that could stand for "any
-- receiver" - a seal binds exactly one recipient - so an existing row gets the
-- most likely name of its receiver instead, which is also what the ADR's own
-- example uses. It is a starting value, not a guess to trust: a receiver that
-- opens deliveries with a cratestack `EnvelopeLayer` is configured with its own
-- audience, and an operator sets that on the endpoint (the console's endpoint
-- screen, or `PATCH /webhook_endpoints/{id}`) when the receiver is moved onto
-- the layer. Until then its deliveries are refused with a 401 and retried on
-- the usual backoff; nothing is lost, because a refused attempt stays
-- `failed`/`dead` and can be replayed once the receiver is fixed. The backfill
-- is capped at the column's 255 characters; a URL with no recognisable origin
-- (the `@uri` validator makes that impossible for a row the API wrote) keeps
-- the whole URL.
--
-- `0001_init` has already been applied to deployed databases and `sms-migrate`
-- records migrations by name, so it cannot stop creating the three secret
-- columns; this forward migration removes them, and `cargo xtask
-- migrations-current` knows that (COLUMNS_CHANGED_BY_LATER in
-- .xtask/src/migrations_current.rs): it takes these four columns out of both
-- sides before comparing `0001_init` with the generator, and checks that this
-- file adds and drops exactly the ones it lists.
--
-- Irreversible by intent: the secrets must not come back from a `down`. See
-- down.sql.

ALTER TABLE webhook_endpoints ADD COLUMN audience TEXT;

UPDATE webhook_endpoints
SET audience = left(coalesce(substring(url FROM '^https?://[^/?#]+'), url), 255);

ALTER TABLE webhook_endpoints ALTER COLUMN audience SET NOT NULL;

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_length_check CHECK (length(audience) BETWEEN 1 AND 255);

ALTER TABLE webhook_endpoints DROP COLUMN secret;

ALTER TABLE webhook_endpoints DROP COLUMN prev_secret;

ALTER TABLE webhook_endpoints DROP COLUMN secret_rotated_at;
