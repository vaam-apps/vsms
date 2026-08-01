ALTER TABLE webhook_attempts DROP CONSTRAINT webhook_attempts_state_enum_check;

ALTER TABLE sender_ids DROP CONSTRAINT sender_ids_value_length_check;

ALTER TABLE routes DROP CONSTRAINT routes_match_class_enum_check;

ALTER TABLE routes DROP CONSTRAINT routes_match_operator_enum_check;

ALTER TABLE providers DROP CONSTRAINT providers_state_enum_check;

ALTER TABLE providers DROP CONSTRAINT providers_kind_enum_check;

ALTER TABLE opt_outs DROP CONSTRAINT opt_outs_source_enum_check;

ALTER TABLE operator_prefix_rules DROP CONSTRAINT operator_prefix_rules_confidence_enum_check;

ALTER TABLE operator_prefix_rules DROP CONSTRAINT operator_prefix_rules_source_enum_check;

ALTER TABLE operator_prefix_rules DROP CONSTRAINT operator_prefix_rules_operator_enum_check;

ALTER TABLE messages DROP CONSTRAINT messages_state_enum_check;

ALTER TABLE messages DROP CONSTRAINT messages_encoding_enum_check;

ALTER TABLE messages DROP CONSTRAINT messages_class_enum_check;

ALTER TABLE messages DROP CONSTRAINT messages_operator_enum_check;

ALTER TABLE message_parts DROP CONSTRAINT message_parts_state_enum_check;

ALTER TABLE jobs DROP CONSTRAINT jobs_state_enum_check;

ALTER TABLE delivery_receipts DROP CONSTRAINT delivery_receipts_network_code_enum_check;

ALTER TABLE delivery_receipts DROP CONSTRAINT delivery_receipts_outcome_enum_check;

DROP INDEX users_email_key;

DROP INDEX users_subject_key;

DROP INDEX sender_ids_value_key;

DROP INDEX roles_key_key;

DROP INDEX providers_key_key;

DROP INDEX opt_outs_msisdn_hash_key;

DROP INDEX operator_prefix_rules_prefix_key;

DROP INDEX oauth_clients_client_id_key;

DROP INDEX apps_slug_key;

DROP INDEX app_clients_client_id_key;

DROP TABLE webhook_endpoints;

DROP TABLE webhook_attempts;

DROP TABLE users;

DROP TABLE sender_ids;

DROP TABLE sender_id_registrations;

DROP TABLE routes;

DROP TABLE roles;

DROP TABLE providers;

DROP TABLE opt_outs;

DROP TABLE operator_prefix_rules;

DROP TABLE oauth_signing_keys;

DROP TABLE oauth_clients;

DROP TABLE messages;

DROP TABLE message_parts;

DROP TABLE jobs;

DROP TABLE delivery_receipts;

DROP TABLE apps;

DROP TABLE app_clients;

