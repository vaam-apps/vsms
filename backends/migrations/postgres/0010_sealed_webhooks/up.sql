-- 0010_sealed_webhooks / up.sql
--
-- VS-8 and VS-8R: an endpoint's deliveries are authenticated in one of two
-- modes, chosen per endpoint and always both available. This migration reshapes
-- `webhook_endpoints` to carry that choice:
--
--   delivery_mode      NEW, NOT NULL, `sealed` or `hmac`: how this endpoint's
--                      deliveries are authenticated
--   audience           NEW, nullable, UNIQUE: the receiver's identity, the audience
--                      a `sealed` delivery's seal binds (cratestack ADR 0006
--                      section 4). Required for `sealed`, unused by `hmac`
--   secret             KEPT, now nullable: the `hmac` mode's signing secret
--   prev_secret        KEPT: the rotation-overlap secret
--   secret_rotated_at  KEPT: when `secret` was last rotated
--
-- Nothing is dropped, and that is the point. vsms serves other apps besides
-- vaam; a receiver that verifies `X-Sms-Signature` today keeps doing so. Every
-- existing row becomes `hmac`, with its secrets exactly as they were, because
-- those receivers verify an HMAC today. `sealed` is opt-in per endpoint, and a
-- deployment will later be able to switch the `hmac` mode off (VSA-1); until then
-- both are always available.
--
-- The two modes are held apart by the database, not by convention:
--
--   sealed -> audience IS NOT NULL and secret, prev_secret are NULL
--             (a sealed endpoint holds no shared secret: there is nothing to leak,
--             to rotate or to forget to delete)
--   hmac   -> secret IS NOT NULL (an HMAC endpoint with no key would sign a
--             delivery with nothing); audience is unused and stays NULL unless an
--             operator set one, so no `hmac` row squats a name a real sealed
--             endpoint may need
--
-- Why `audience` is UNIQUE, and never `vsms`. One gateway key signs every app's
-- sealed deliveries, so the audience is what tells one receiver's seals from
-- another's. Were it free, whoever holds `webhook:manage` could create an
-- endpoint that names a victim's audience and points at a URL of their own,
-- collect seals the victim's receiver would open, and replay them inside the
-- skew window. The unique index closes the name (an audience is taken once,
-- first come; Postgres allows any number of NULLs, so `hmac` rows do not
-- collide), and the delivery envelope's `appId` and `endpointId` close the rest:
-- a receiver compares them with its own (`sms-webhook`'s `lib.md`, "What a
-- receiver configures"). `vsms` is the name the gateway's own routes answer to,
-- so a receiver that used it in both directions would give up the protection
-- against reflecting a request back; the CHECK refuses it.
--
-- There is no backfill of `audience`. An earlier draft of this migration derived
-- one from each URL's origin for every existing row. An `hmac` row needs none,
-- and a guessed one would occupy a name a real endpoint may need (two apps on
-- one host collide on it, and the loser would be renamed with an id suffix nobody
-- chose). A sealed endpoint is created with an audience its receiver was
-- configured with, or an `hmac` endpoint is switched to `sealed` with one.
--
-- The generator emits the unique index, the enum CHECK on `delivery_mode` and
-- the length CHECK on `audience` (`@unique`, the enum, `@length` with
-- `@db_enforce`), and `cargo xtask migrations-current` requires them verbatim
-- below. It cannot emit the rest, so they are written here, listed in
-- HAND_WRITTEN_COLUMN_CONSTRAINTS in `.xtask/src/migrations_current.rs`, and
-- proved by `sealed_webhooks_migration_live_postgres.rs`:
--
--   webhook_endpoints_audience_not_vsms_check                    (@db_enforce does nothing for a @regex)
--   webhook_endpoints_audience_required_for_sealed_check         (a cross-column rule)
--   webhook_endpoints_delivery_mode_hmac_needs_secret_check      (a cross-column rule)
--   webhook_endpoints_delivery_mode_sealed_holds_no_secret_check (a cross-column rule)
--
-- `0001_init` has already been applied to deployed databases and `sms-migrate`
-- records migrations by name, so it cannot stop creating `secret NOT NULL`; this
-- forward migration relaxes it, and `cargo xtask migrations-current` knows that
-- (COLUMNS_CHANGED_BY_LATER in .xtask/src/migrations_current.rs): it takes the
-- columns this file adds or alters out of both sides before comparing `0001_init`
-- with the generator, and checks that this file adds and alters exactly the ones
-- it lists.
--
-- The secrets are in more places than the column. `webhook_endpoints` is an
-- `@@audit` model, and `cratestack_audit` holds a snapshot of the whole row at
-- every create and update. Until this migration's schema `secret` and
-- `prevSecret` were not `@sensitive`, so those snapshots carry every HMAC secret
-- this table held up to now in plaintext, and the anchor chain `anchor_audit`
-- writes over them exists precisely so that history cannot be rewritten: this
-- migration does not scrub them and no later one should try. From here on the
-- snapshots redact both. Rotate each `hmac` endpoint's secret once after the
-- deploy (`rotateWebhookSecret`), so the secret in use is not one the audit
-- history holds.
--
-- Reversible, unlike the first draft: see down.sql.

ALTER TABLE webhook_endpoints ADD COLUMN delivery_mode TEXT;

UPDATE webhook_endpoints SET delivery_mode = 'hmac';

ALTER TABLE webhook_endpoints ALTER COLUMN delivery_mode SET NOT NULL;

ALTER TABLE webhook_endpoints ADD COLUMN audience TEXT;

ALTER TABLE webhook_endpoints ALTER COLUMN secret DROP NOT NULL;

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_delivery_mode_enum_check CHECK (delivery_mode IN ('sealed', 'hmac'));

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_length_check CHECK (length(audience) BETWEEN 1 AND 255);

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_not_vsms_check CHECK (audience <> 'vsms');

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_audience_required_for_sealed_check CHECK (delivery_mode <> 'sealed' OR audience IS NOT NULL);

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_delivery_mode_hmac_needs_secret_check CHECK (delivery_mode <> 'hmac' OR secret IS NOT NULL);

ALTER TABLE webhook_endpoints ADD CONSTRAINT webhook_endpoints_delivery_mode_sealed_holds_no_secret_check CHECK (delivery_mode <> 'sealed' OR (secret IS NULL AND prev_secret IS NULL AND secret_rotated_at IS NULL));

CREATE UNIQUE INDEX webhook_endpoints_audience_key ON webhook_endpoints (audience);
