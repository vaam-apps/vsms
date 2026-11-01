-- 0014_optout_read_scope / down.sql
--
-- `sms-migrate` never runs this file (forward-only); it records what undoing
-- the change means. Takes `optout:read` back from the built-in `auditor`, the
-- rest of the set kept and sentinel-packed (' ' when nothing is left).
UPDATE roles
   SET permissions = ' ' || coalesce(
           nullif(array_to_string(
               array_remove(regexp_split_to_array(btrim(permissions, E' \t\r\n'), '\s+'), 'optout:read'),
               ' '), '') || ' ',
           ''),
       version = version + 1
 WHERE builtin
   AND key = 'auditor'
   AND 'optout:read' = ANY(regexp_split_to_array(permissions, '\s+'));
