-- 0010_sealed_webhooks / down.sql
--
-- Reversible, because nothing was dropped (VS-8R). One thing can stand in its
-- way: a `sealed` endpoint holds no HMAC secret, and `0001_init` has `secret NOT
-- NULL`. A secret minted here would be one no receiver knows, so the rollback
-- refuses instead and names how many endpoints are in the way. Switch them back
-- to `hmac` (which mints a secret the receiver is then given) or delete them,
-- and run this again.
--
-- `sms-migrate` never runs a down; this is the tested record of how to go back.

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

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_sealed_holds_no_secret_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_hmac_needs_secret_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_required_for_sealed_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_not_vsms_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_audience_length_check;

ALTER TABLE webhook_endpoints DROP CONSTRAINT webhook_endpoints_delivery_mode_enum_check;

ALTER TABLE webhook_endpoints ALTER COLUMN secret SET NOT NULL;

ALTER TABLE webhook_endpoints DROP COLUMN audience;

ALTER TABLE webhook_endpoints DROP COLUMN delivery_mode;
