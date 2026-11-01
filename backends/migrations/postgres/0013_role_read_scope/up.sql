-- 0013_role_read_scope / up.sql
--
-- Owner's decision after #469 (reading message data needs `sms:read`): the
-- built-in roles that keep `sms:read` are owner, admin, operator and auditor.
-- `developer` and `support` lose it, and a zero-permission `pending` role
-- exists for a first single-sign-on sign-in to wait in (#458's
-- `human_login.sso.jit_role`).
--
-- DATA ONLY. No table, column, constraint or policy changes, so the contract
-- digest and `cargo xtask migrations-current` do not move. The seed in
-- 0002_bootstrap (generated from docs/architecture.md section 2.10) is frozen
-- like every shipped migration; this file is how a database that has run it
-- gets from the old seed to the new one, and a fresh database runs both.
--
-- Raw SQL writes no @@audit row for the role changes below (the model's audit
-- trail is the generated client's): this file and the migration record are the
-- trail.
--
-- 1. A `pending` role that already holds permissions stops the migration. It was
--    made by an owner (no earlier migration seeds it), and `jit_role: pending`
--    would hand everything it holds to every first sign-in, so the migration
--    refuses and rolls back (sms-migrate and `psql -1` run it in one
--    transaction) rather than keep or overwrite it. Rename or empty the role
--    first (docs/runbooks/sso.adoc). An empty one is accepted and kept as it is.
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM roles
                WHERE key = 'pending' AND btrim(permissions, E' \t\r\n') <> '') THEN
        RAISE EXCEPTION '0013_role_read_scope: a role keyed pending already exists and holds permissions; rename or empty it first (jit_role: pending would hand them to every first sign-in)';
    END IF;
END
$$;

-- 2. `developer` and `support` lose `sms:read`.
--
-- `roles.permissions` is a whitespace-separated set; the seed and the console's
-- editor write it sentinel-packed (`sms_core::pack`: ' a b c ', ' ' when empty),
-- and the Rust side reads it with `split_whitespace`, so this reads it the same
-- way and does not rely on the packing: the token is found among the
-- `\s+`-split words (never as part of `xsms:read` or `sms:readonly`), removed,
-- and the rest is written back sentinel-packed. A row with a tab, a doubled
-- space or no sentinel at all loses the token too, and is rewritten packed. An
-- emptied set is exactly ' '. The WHERE is `builtin` and the two keys, so a
-- custom role an owner made (whatever it is called, whatever it holds) is
-- untouched, and the token test makes a second run (or a built-in role already
-- edited to lack it) match nothing: no row, `version` or `updated_at` moves.
--
-- `version` is bumped on every row changed, as the console's role editor does
-- through `@version`, so an editor holding the old copy gets a 412 instead of
-- writing the permission back over this.
UPDATE roles
   SET permissions = ' ' || coalesce(
           nullif(array_to_string(
               array_remove(regexp_split_to_array(btrim(permissions, E' \t\r\n'), '\s+'), 'sms:read'),
               ' '), '') || ' ',
           ''),
       version = version + 1
 WHERE builtin
   AND key IN ('developer', 'support')
   AND 'sms:read' = ANY(regexp_split_to_array(permissions, '\s+'));

-- 3. `pending`: no permission at all (' ' is `pack` of nothing). `ON CONFLICT
--    DO NOTHING` makes it idempotent: the only row that can already have this
--    key at this point is an empty one (step 1), which is kept as it is.
INSERT INTO roles (key, label, description, builtin, permissions, version) VALUES
    ('pending', 'Pending',
     'A first sign-in waits here until an owner or admin promotes it. Holds no permission.',
     true, ' ', 0)
ON CONFLICT (key) DO NOTHING;
