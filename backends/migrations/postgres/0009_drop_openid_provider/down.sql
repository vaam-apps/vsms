-- 0009_drop_openid_provider / down.sql
--
-- Forward-only, like the generator's own stub for a lossy migration. The rows
-- this migration drops are a signing key's private half, password hashes and
-- the OP's client registry: a `down` that recreated the empty tables would
-- restore nothing a caller could use, and one that could restore the rows
-- would resurrect credentials that were deleted on purpose. To go back,
-- redeploy the release before VS-4 against a restored backup.
--
-- `sms-migrate` never runs a down; this is the record of why there is none.

DO $$ BEGIN RAISE EXCEPTION 'destructive migration (VS-4: the OpenID Provider tables); restore from a backup taken before 0009'; END $$;
