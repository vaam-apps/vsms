\set ON_ERROR_STOP 1
BEGIN;
-- #59: apps.version is now NOT NULL with no SQL DEFAULT (the ORM seeds it
-- server-side; this script writes raw SQL and has to supply it itself).
INSERT INTO apps (name, slug, description, monthly_quota, ip_allowlist, transliterate_to_gsm7, version)
VALUES ('probe','probe',NULL,0,' ',false,0);
INSERT INTO messages (app_id, msisdn, msisdn_hash, operator, sender_id_value, class,
                      priority, body_hash, body_length, encoding, segments, max_attempts, expires_at)
SELECT id,'+237690000000','h','orange','VYMALO','otp',900,'bh',0,'gsm7',1,2, now()+interval '15 min'
FROM apps WHERE slug='probe';

-- id shape must satisfy the Cuid guard used by REST query filters: [a-z0-9]{2,32}
DO $$ DECLARE i TEXT; BEGIN
  SELECT id INTO i FROM messages LIMIT 1;
  ASSERT i ~ '^[a-z0-9]{2,32}$', format('cs_cuid() produced a non-Cuid id: %s', i);
END $$;

UPDATE messages SET state='queued'    WHERE state='accepted';
UPDATE messages SET state='routed'    WHERE state='queued';
UPDATE messages SET state='submitted' WHERE state='routed';
DO $$ BEGIN
  ASSERT (SELECT submitted_at IS NOT NULL FROM messages), 'submitted_at was not auto-stamped';
  ASSERT (SELECT finalized_at IS NULL     FROM messages), 'finalized_at stamped too early';
END $$;

-- illegal: submitted has no edge back to accepted
DO $$ BEGIN
  BEGIN
    UPDATE messages SET state='accepted' WHERE state='submitted';
    RAISE EXCEPTION 'illegal transition submitted->accepted was ALLOWED';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

UPDATE messages SET state='delivered' WHERE state='submitted';
DO $$ BEGIN
  ASSERT (SELECT finalized_at IS NOT NULL FROM messages), 'finalized_at was not auto-stamped';
END $$;

-- illegal: delivered is terminal (no outgoing rows in the transition table)
DO $$ BEGIN
  BEGIN
    UPDATE messages SET state='queued' WHERE state='delivered';
    RAISE EXCEPTION 'terminal state delivered was allowed to transition';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

-- jobs: running -> pending is the crash-reclaim edge and must be legal
INSERT INTO jobs (kind, dedupe_key, payload, priority, run_at, max_attempts)
VALUES ('probe_job', NULL, '{}', 10, now(), 5);
UPDATE jobs SET state='running' WHERE state='pending';
UPDATE jobs SET state='pending' WHERE state='running';
DO $$ BEGIN
  BEGIN
    UPDATE jobs SET state='succeeded' WHERE state='pending';
    RAISE EXCEPTION 'illegal job transition pending->succeeded was ALLOWED';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

-- #56: requeueJob's own edge. Walk the job to dead (running -> failed ->
-- dead, the exhausted-attempts path apply_failure takes), confirm dead is
-- otherwise a dead end, then confirm dead -> pending — and only that edge —
-- is what gets it out.
UPDATE jobs SET state='running' WHERE state='pending';
UPDATE jobs SET state='failed'  WHERE state='running';
UPDATE jobs SET state='dead'    WHERE state='failed';
DO $$ BEGIN
  BEGIN
    UPDATE jobs SET state='running' WHERE state='dead';
    RAISE EXCEPTION 'dead was allowed to transition straight back into running';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;
UPDATE jobs SET state='pending' WHERE state='dead';
DO $$ BEGIN
  ASSERT (SELECT state='pending' FROM jobs), 'dead -> pending (requeue, #56) was not accepted';
END $$;

-- updated_at trigger
UPDATE apps SET name='probe2' WHERE slug='probe';
DO $$ BEGIN
  ASSERT (SELECT updated_at > created_at FROM apps WHERE slug='probe'),
         'touch_updated_at did not fire';
END $$;

-- webhook dedupe index must reject the second identical (endpoint, aggregate, type)
-- #59: webhook_endpoints.version is now NOT NULL with no SQL DEFAULT.
INSERT INTO webhook_endpoints (app_id, url, event_types, secret, mask_recipient, max_attempts, version)
SELECT id, 'https://example.test/hook', ' message.delivered ', 's', true, 8, 0 FROM apps WHERE slug='probe';
INSERT INTO webhook_attempts (endpoint_id, source_event_id, aggregate_id, event_type, payload)
SELECT e.id, gen_random_uuid(), m.id, 'message.delivered', '{}' FROM webhook_endpoints e, messages m;
DO $$ BEGIN
  BEGIN
    INSERT INTO webhook_attempts (endpoint_id, source_event_id, aggregate_id, event_type, payload)
    SELECT e.id, gen_random_uuid(), m.id, 'message.delivered', '{}' FROM webhook_endpoints e, messages m;
    RAISE EXCEPTION 'webhook_attempts_dedupe did not reject the duplicate';
  EXCEPTION WHEN unique_violation THEN NULL;
  END;
END $$;

-- webhook attempts: pending -> delivering -> succeeded, with delivered_at
-- auto-stamped by the trigger (#40) — same convention as messages'
-- finalized_at / jobs' finished_at above.
UPDATE webhook_attempts SET state='delivering' WHERE state='pending';
DO $$ BEGIN
  ASSERT (SELECT delivered_at IS NULL FROM webhook_attempts), 'delivered_at stamped too early';
END $$;
UPDATE webhook_attempts SET state='succeeded' WHERE state='delivering';
DO $$ BEGIN
  ASSERT (SELECT delivered_at IS NOT NULL FROM webhook_attempts), 'delivered_at was not auto-stamped';
END $$;

-- illegal: succeeded is terminal
DO $$ BEGIN
  BEGIN
    UPDATE webhook_attempts SET state='pending' WHERE state='succeeded';
    RAISE EXCEPTION 'terminal state succeeded was allowed to transition';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

-- failed -> delivering is the retry-after-backoff edge and must be legal;
-- delivering -> dead (max attempts, or 410 Gone) must be too. A second,
-- fresh row — the first is already terminal (succeeded) above, and
-- webhook_attempts_dedupe (endpoint_id, aggregate_id, event_type) means a
-- second row needs its own event_type to coexist with the first.
INSERT INTO webhook_attempts (endpoint_id, source_event_id, aggregate_id, event_type, payload, state)
SELECT e.id, gen_random_uuid(), m.id, 'message.submitted', '{}', 'failed'
FROM webhook_endpoints e, messages m;
UPDATE webhook_attempts SET state='delivering' WHERE state='failed';
UPDATE webhook_attempts SET state='dead' WHERE state='delivering' AND event_type='message.submitted';
DO $$ BEGIN
  BEGIN
    UPDATE webhook_attempts SET state='delivering' WHERE state='dead';
    RAISE EXCEPTION 'dead was allowed to transition straight back into delivering';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

-- #43: replay re-fires a stuck attempt. dead -> pending and failed ->
-- pending are the two edges replayWebhookAttempt proposes; nothing else in
-- this codebase ever proposes either. delivering -> pending stays illegal
-- (a replay must never be able to race an attempt a worker is actively
-- delivering), and succeeded -> pending still doesn't exist either — see
-- "illegal: succeeded is terminal" above, unaffected by this section.
UPDATE webhook_attempts SET state='pending' WHERE state='dead' AND event_type='message.submitted';
DO $$ BEGIN
  ASSERT (SELECT state='pending' FROM webhook_attempts WHERE event_type='message.submitted'),
         'dead -> pending (replay) was not accepted';
END $$;

UPDATE webhook_attempts SET state='delivering' WHERE state='pending' AND event_type='message.submitted';
UPDATE webhook_attempts SET state='failed' WHERE state='delivering' AND event_type='message.submitted';
UPDATE webhook_attempts SET state='pending' WHERE state='failed' AND event_type='message.submitted';
DO $$ BEGIN
  ASSERT (SELECT state='pending' FROM webhook_attempts WHERE event_type='message.submitted'),
         'failed -> pending (replay) was not accepted';
END $$;

UPDATE webhook_attempts SET state='delivering' WHERE state='pending' AND event_type='message.submitted';
DO $$ BEGIN
  BEGIN
    UPDATE webhook_attempts SET state='pending' WHERE state='delivering' AND event_type='message.submitted';
    RAISE EXCEPTION 'delivering -> pending must not be legal (would race an active delivery)';
  EXCEPTION WHEN SQLSTATE 'SM001' THEN NULL;
  END;
END $$;

ROLLBACK;
\echo 'ALL ASSERTIONS PASSED'
