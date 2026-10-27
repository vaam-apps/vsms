-- 0010_sealed_webhooks / down.sql
--
-- Forward-only. The columns this migration drops held every endpoint's HMAC
-- signing secret and its rotation-overlap secret: a `down` that recreated them
-- empty would restore nothing a receiver could verify against, and one that
-- could restore the values would resurrect shared secrets that were deleted on
-- purpose. To go back, redeploy the release before VS-8 against a restored
-- backup.
--
-- `sms-migrate` never runs a down; this is the record of why there is none.

DO $$ BEGIN RAISE EXCEPTION 'destructive migration (VS-8: the webhook signing secrets); restore from a backup taken before 0010'; END $$;
