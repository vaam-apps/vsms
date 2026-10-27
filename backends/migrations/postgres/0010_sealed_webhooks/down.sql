-- 0010_sealed_webhooks / down.sql
--
-- Reversible, because nothing was dropped (VS-8R). One thing can stand in its
-- way: a `sealed` endpoint holds no HMAC secret, and `0001_init` has `secret NOT
-- NULL`. A secret minted here would be one no receiver knows, so the rollback
-- refuses instead and names how many endpoints are in the way. Switch each to
-- `hmac` with `switchWebhookDeliveryMode`, which mints a new secret that its
-- receiver must then be given (a receiver of sealed deliveries holds none to
-- reuse), or delete them, and run this again.
--
-- It is one transaction, so a refusal, or a statement that fails halfway, leaves
-- the migration fully applied rather than half undone, whichever way the file is
-- run (`psql -f` without `ON_ERROR_STOP` carries on past an error; inside the
-- transaction everything after it is rolled back).
--
-- `sms-migrate` never runs a down; this is the tested record of how to go back.

BEGIN;

DO $$
DECLARE
    sealed_count BIGINT;
BEGIN
    SELECT count(*) INTO sealed_count FROM webhook_endpoints WHERE delivery_mode = 'sealed';
    IF sealed_count > 0 THEN
        RAISE EXCEPTION 'cannot roll back 0010: % sealed webhook endpoint(s) hold no HMAC secret; switch them to hmac or delete them first', sealed_count;
    END IF;
END $$;

DROP INDEX webhook_endpoints_audience_key;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_url_sealable_for_sealed_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_sealed_holds_no_secret_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_hmac_needs_secret_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_required_for_sealed_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_only_for_sealed_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_not_vsms_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_length_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_enum_check;

ALTER TABLE webhook_endpoints ALTER COLUMN secret SET NOT NULL;

ALTER TABLE webhook_endpoints DROP COLUMN audience;

ALTER TABLE webhook_endpoints DROP COLUMN delivery_mode;

COMMIT;
