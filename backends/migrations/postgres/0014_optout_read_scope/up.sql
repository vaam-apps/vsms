-- 0014_optout_read_scope / up.sql
--
-- Owner's decision after #470 (2026-11-01): a person reads opt-outs and
-- consent records only with `optout:read` or `optout:manage` (the `OptOut` and
-- `ConsentRecord` read policies ask the claim `optOutRead`). The built-in roles
-- that manage opt-outs (owner, admin, operator, support) already hold
-- `optout:manage` and so lose nothing; `developer` and `pending` hold neither
-- and no longer read them. The built-in `auditor`, which reads everything
-- else and holds neither, is given `optout:read`.
--
-- DATA ONLY. The policy change is in schemas/vsms.cstack (no DDL), and no
-- table, column or constraint changes here, so the contract digest and
-- `cargo xtask migrations-current` do not move. The seed in 0002_bootstrap
-- (generated from docs/architecture.md section 2.10) is frozen like every
-- shipped migration; this file is how a database that has run it, and 0013,
-- gets to the new seed, and a fresh database runs all of them.
--
-- Raw SQL writes no @@audit row for the role change below (the model's audit
-- trail is the generated client's): this file and the migration record are the
-- trail.
--
-- `roles.permissions` is a whitespace-separated set; the seed and the console's
-- editor write it sentinel-packed (`sms_core::pack`: ' a b c '), and the Rust
-- side reads it with `split_whitespace`, so this reads it the same way as 0013
-- and does not rely on the packing: the token is looked for among the
-- `\s+`-split words (never as part of `xoptout:read` or `optout:readonly`),
-- appended when absent, and the set written back sentinel-packed. A row with a
-- tab, a doubled space or no sentinel at all gains the token too, and is
-- rewritten packed. The WHERE is `builtin` and the one key, so a custom role an
-- owner made (whatever it is called, whatever it holds) is untouched, and the
-- token test makes a second run (or an `auditor` already edited to hold it)
-- match nothing: no row, `version` or `updated_at` moves.
--
-- `version` is bumped on the row changed, as the console's role editor does
-- through `@version`, so an editor holding the old copy gets a 412 instead of
-- writing the permission away.
UPDATE roles
   SET permissions = ' ' || array_to_string(
           array_append(
               array_remove(regexp_split_to_array(btrim(permissions, E' \t\r\n'), '\s+'), ''),
               'optout:read'),
           ' ') || ' ',
       version = version + 1
 WHERE builtin
   AND key = 'auditor'
   AND NOT ('optout:read' = ANY(regexp_split_to_array(permissions, '\s+')));
