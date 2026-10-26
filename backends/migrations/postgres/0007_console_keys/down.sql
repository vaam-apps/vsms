-- 0007_console_keys / down.sql
--
-- Drops the ConsoleKey table. Any registered public keys are lost; they are
-- recoverable by pairing again (`registerConsoleKey`) or by
-- `sms-gateway pair-console-key` with the same public key (the private halves
-- never reached vsms). Nothing verifies against this table until VS-3, so
-- dropping it cannot lock a caller out that this migration let in.

ALTER TABLE console_keys DROP CONSTRAINT console_keys_user_id_fkey;

ALTER TABLE console_keys DROP CONSTRAINT console_keys_alg_enum_check;

DROP INDEX console_keys_thumbprint_key;

DROP TABLE console_keys;
