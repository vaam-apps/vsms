-- 0015_delivery_lookups / down.sql
--
-- `sms-migrate` never runs this file (forward-only); it records what undoing the
-- change means. Drops the DeliveryLookup table: every recorded lookup is lost,
-- and with it the account of who looked at which delivery status (the
-- `cratestack_audit` rows the model's @@audit wrote stay, and stay anchored).
-- Roll the procedure back first: while it is served, a missing table makes every
-- lookup fail closed, by design.

DROP INDEX delivery_lookups_actor_created_idx;

ALTER TABLE delivery_lookups DROP CONSTRAINT delivery_lookups_key_kind_enum_check;

DROP TABLE delivery_lookups;
