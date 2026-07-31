import re, sys
ts = "App AppClient OauthClient SenderId SenderIdRegistration Provider Route Message MessagePart Job OptOut WebhookEndpoint User Role".split()
DOC = 'docs/architecture.md'
def tbl(n):
    s = re.sub(r'(?<!^)(?=[A-Z])', '_', n).lower()
    return s+'es' if s.endswith('s') else s+'s'
tables=[tbl(n) for n in ts]
doc=open(DOC).read()
sec=doc[doc.index('### 2.10 Hand-written SQL'):doc.index('## 3. The send path')]
raw='\n'.join(b.rstrip()+'\n' for b in re.findall(r'```sql\n(.*?)```', sec, re.S))
raw=raw.replace("""ALTER TABLE apps ALTER COLUMN created_at SET DEFAULT now(),
                 ALTER COLUMN updated_at SET DEFAULT now();
-- ... repeat for every table using @use(Timestamps)""",
"\n".join(f"ALTER TABLE {t} ALTER COLUMN created_at SET DEFAULT now(),\n{' '*12}ALTER COLUMN updated_at SET DEFAULT now();" for t in tables))
raw=raw.replace("""CREATE TRIGGER apps_touch BEFORE UPDATE ON apps
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
-- ... repeat for every table using @use(Timestamps)""",
"\n".join(f"CREATE TRIGGER {t}_touch BEFORE UPDATE ON {t}\n    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();" for t in tables))
assert 'repeat for every table' not in raw
hdr="""-- 0002_bootstrap / up.sql
--
-- Everything cratestack-migrate does not emit: identifier and timestamp
-- defaults, the updated_at trigger, the two state machines, non-unique and
-- partial indexes, and foreign keys.
--
-- Generated from docs/architecture.md section 2.10 by ci/gen-bootstrap-sql.py.
-- Do not hand-edit: edit the document, regenerate, and commit both.

"""
open(sys.argv[1],'w').write(hdr+raw)
print(f"{len(raw.splitlines())} lines, {len(tables)} timestamped tables")
