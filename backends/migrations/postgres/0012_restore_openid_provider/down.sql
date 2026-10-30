-- 0012_restore_openid_provider / down.sql
--
-- Drops the five tables 0012 created, and nothing else (0009 stays dropped).
-- Reversible while the tables are empty, which is how 0012 leaves them and how
-- they stay until VSA-3 and VSA-4 write to them. Once they are not, a rollback
-- would delete OAuth clients, the OP's signing keys, password hashes and SSO
-- sign-ins in flight, and unlike 0009's `down` there is nothing to refuse
-- *restoring*: what it refuses is a silent loss. So it counts the rows first and
-- refuses, naming the counts. Delete the rows on purpose (or take a backup, which
-- is the only way back) and run it again.
--
-- The statements after the guard are the emitter's own `down.sql` for these five
-- tables, byte for byte, which `cargo xtask migrations-current` requires
-- (ADDED_BY_LATER). Dropping a table takes its indexes, CHECK constraints,
-- defaults, triggers and foreign keys with it; the explicit ones before the
-- `DROP TABLE`s are the emitter's, kept so the file is its output.
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
    clients BIGINT;
    signing_keys BIGINT;
    assertions BIGINT;
    credentials BIGINT;
    transactions BIGINT;
BEGIN
    SELECT count(*) INTO clients FROM oauth_clients;
    SELECT count(*) INTO signing_keys FROM oauth_signing_keys;
    SELECT count(*) INTO assertions FROM client_assertions;
    SELECT count(*) INTO credentials FROM user_credentials;
    SELECT count(*) INTO transactions FROM sso_transactions;
    IF clients + signing_keys + assertions + credentials + transactions > 0 THEN
        RAISE EXCEPTION 'cannot roll back 0012: oauth_clients=%, oauth_signing_keys=%, client_assertions=%, user_credentials=%, sso_transactions=% row(s) would be deleted; delete them on purpose, or restore from a backup, and run this again', clients, signing_keys, assertions, credentials, transactions;
    END IF;
END $$;

ALTER TABLE user_credentials DROP CONSTRAINT user_credentials_user_id_fkey;

ALTER TABLE oauth_clients DROP CONSTRAINT oauth_clients_app_client_id_fkey;

ALTER TABLE oauth_clients DROP CONSTRAINT oauth_clients_token_endpoint_auth_method_enum_check;

DROP INDEX user_credentials_user_id_key;

DROP INDEX sso_transactions_state_hash_key;

DROP INDEX oauth_clients_client_id_key;

DROP INDEX client_assertions_jti_key;

DROP TABLE user_credentials;

DROP TABLE sso_transactions;

DROP TABLE oauth_signing_keys;

DROP TABLE oauth_clients;

DROP TABLE client_assertions;

COMMIT;
