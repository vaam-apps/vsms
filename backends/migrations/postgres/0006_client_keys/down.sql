-- 0006_client_keys / down.sql
--
-- Drops the ClientKey table. Any registered public keys are lost; they are
-- recoverable by re-running `sms-gateway register-client` with the same
-- public key (the private halves never reached vsms). Nothing verifies
-- against this table until VS-3, so dropping it cannot lock a caller out
-- that this migration let in.

ALTER TABLE client_keys DROP CONSTRAINT client_keys_app_client_id_fkey;

ALTER TABLE client_keys DROP CONSTRAINT client_keys_alg_enum_check;

DROP INDEX client_keys_thumbprint_key;

DROP TABLE client_keys;
