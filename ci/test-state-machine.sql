\set ON_ERROR_STOP 1
BEGIN;
INSERT INTO apps (name, slug, description, monthly_quota, ip_allowlist, transliterate_to_gsm7)
VALUES ('probe','probe',NULL,0,' ',false);
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

-- updated_at trigger
UPDATE apps SET name='probe2' WHERE slug='probe';
DO $$ BEGIN
  ASSERT (SELECT updated_at > created_at FROM apps WHERE slug='probe'),
         'touch_updated_at did not fire';
END $$;

-- webhook dedupe index must reject the second identical (endpoint, aggregate, type)
INSERT INTO webhook_endpoints (app_id, url, event_types, secret, mask_recipient, max_attempts)
SELECT id, 'https://example.test/hook', ' message.delivered ', 's', true, 8 FROM apps WHERE slug='probe';
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

ROLLBACK;
\echo 'ALL ASSERTIONS PASSED'
