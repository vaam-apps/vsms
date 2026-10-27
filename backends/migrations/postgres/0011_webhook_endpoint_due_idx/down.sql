-- 0011_webhook_endpoint_due_idx / down.sql
--
-- Drops the index. Nothing else depends on it: a keyless `hooks` worker's claim
-- still answers without it, by reading `webhook_due_idx` across every endpoint
-- (the slow plan this migration exists to avoid), so a rollback loses speed and
-- no row.

DROP INDEX webhook_endpoint_due_idx;
