ALTER TABLE sender_ids DROP CONSTRAINT sender_ids_value_length_check;

DROP INDEX users_email_key;

DROP INDEX users_subject_key;

DROP INDEX sender_ids_value_key;

DROP INDEX roles_key_key;

DROP INDEX providers_key_key;

DROP INDEX opt_outs_msisdn_hash_key;

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

DROP TABLE oauth_clients;

DROP TABLE messages;

DROP TABLE message_parts;

DROP TABLE jobs;

DROP TABLE delivery_receipts;

DROP TABLE apps;

DROP TABLE app_clients;

DROP TYPE provider_state;

DROP TYPE provider_kind;

DROP TYPE opt_out_source;

DROP TYPE operator_code;

DROP TYPE message_state;

DROP TYPE message_class;

DROP TYPE job_state;

DROP TYPE encoding;

DROP TYPE delivery_outcome;

DROP TYPE attempt_state;

