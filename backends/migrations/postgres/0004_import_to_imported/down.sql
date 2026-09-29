ALTER TABLE opt_outs DROP CONSTRAINT opt_outs_source_enum_check;

ALTER TABLE consent_records DROP CONSTRAINT consent_records_channel_enum_check;

UPDATE opt_outs SET source = 'import' WHERE source = 'imported';

UPDATE consent_records SET channel = 'import' WHERE channel = 'imported';

ALTER TABLE opt_outs ADD CONSTRAINT opt_outs_source_enum_check CHECK (source IN ('inbound_stop', 'admin', 'import', 'operator'));

ALTER TABLE consent_records ADD CONSTRAINT consent_records_channel_enum_check CHECK (channel IN ('web_form', 'api', 'ivr', 'paper_form', 'verbal', 'sms_keyword', 'import', 'admin'));
