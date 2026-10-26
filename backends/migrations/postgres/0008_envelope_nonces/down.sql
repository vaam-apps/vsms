-- 0008_envelope_nonces / down.sql
--
-- Drops the EnvelopeNonce table. The rows are a replay cache, so nothing is
-- lost that matters beyond the freshness window (five minutes of skew either
-- side); but while the envelope layer is mounted, a request sealed before the
-- drop could be delivered once more inside that window, so roll the layer back
-- first.

DROP INDEX envelope_nonces_nonce_key;

DROP TABLE envelope_nonces;
