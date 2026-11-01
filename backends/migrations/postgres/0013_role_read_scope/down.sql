-- 0013_role_read_scope / down.sql
--
-- `sms-migrate` never runs this file (forward-only); it records what undoing
-- the change means. Gives `developer` and `support` `sms:read` back, appended
-- (the packed format ends with a space), and removes `pending` unless a user
-- still holds it, in which case the row stays: `users.role_key` references it.
UPDATE roles
   SET permissions = permissions || 'sms:read ',
       version     = version + 1
 WHERE builtin
   AND key IN ('developer', 'support')
   AND permissions NOT LIKE '% sms:read %';

DELETE FROM roles
 WHERE key = 'pending'
   AND builtin
   AND NOT EXISTS (SELECT 1 FROM users WHERE users.role_key = 'pending');
