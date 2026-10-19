-- 0005_sso_federation / down.sql
--
-- Drops the SSO tables. Any FederatedLink rows are lost - users keep their
-- local `User` rows and can still sign in by password if they have a
-- `UserCredential`; JIT-provisioned users have none and would have to be
-- given one (`sms-gateway provision-user` refuses an existing email, so the
-- row would have to be removed or re-credentialed by hand first).

DROP INDEX sso_transactions_expiry_idx;

DROP INDEX federated_links_user_id_idx;

DROP INDEX federated_links_issuer_subject_key;

ALTER TABLE federated_links DROP CONSTRAINT federated_links_user_id_fkey;

DROP INDEX sso_transactions_state_hash_key;

DROP TABLE sso_transactions;

DROP TABLE federated_links;
