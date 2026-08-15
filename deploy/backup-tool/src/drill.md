`restore-drill` — #69's actual gate: "backups that have never been
restored are not backups." Seeds a marker row, records an exact row
count for every table, backs up, destroys every object in the target
database, restores, and diffs both before vs after. A non-zero exit
means the backup *mechanism* is broken, not this tool.

Direct port of the old `deploy/restore-drill.sh` — same six steps,
same destructive `DROP SCHEMA public CASCADE`, same refusal to run
without an explicit confirmation flag. **Never point this at a
database with data you care about.**
