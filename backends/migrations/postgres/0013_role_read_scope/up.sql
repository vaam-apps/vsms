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
-- `roles.permissions` is sentinel-packed in `sms_core::pack`'s format: a leading
-- space, then each permission followed by a space (' a b c '; ' ' when empty).
-- Removing a permission therefore removes the permission and ONE of its two
-- neighbouring spaces, which cannot leave a double space or a dangling
-- separator. The lookahead keeps the trailing space for the next token (and for
-- a duplicate of the permission, which `g` then removes too); the leading space
-- in the pattern keeps `xsms:read` and `sms:readonly` out of reach. The WHERE
-- is `builtin` and the two keys, so a custom role an owner made (whatever it is
-- called, whatever it holds) is untouched, and so is a built-in role an owner
-- already edited to lack the permission: running this twice changes nothing the
-- second time.
--
-- `version` is bumped on every row changed, as the console's role editor does
-- through `@version`, so an editor holding the old copy gets a 412 instead of
-- writing the permission back over this.
UPDATE roles
   SET permissions = regexp_replace(permissions, ' sms:read(?= )', '', 'g'),
       version     = version + 1
 WHERE builtin
   AND key IN ('developer', 'support')
   AND permissions LIKE '% sms:read %';

-- `pending`: no permission at all (' ' is `pack` of nothing). `ON CONFLICT DO
-- NOTHING` makes it idempotent and leaves alone a role an owner already
-- created under that key (it is theirs; docs/runbooks/sso.adoc says to check
-- it holds nothing before relying on it).
INSERT INTO roles (key, label, description, builtin, permissions, version) VALUES
    ('pending', 'Pending',
     'A first sign-in waits here until an owner or admin promotes it. Holds no permission.',
     true, ' ', 0)
ON CONFLICT (key) DO NOTHING;
