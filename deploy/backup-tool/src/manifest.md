The backup manifest — a small, deliberately unencrypted JSON file
written next to every `.dump`. Field names and shapes are unchanged
from the bash-era `backup.sh` (`docs/runbooks/backup-restore.adoc`'s own
documented shape), on purpose: a real deployment may already have
backups sitting in a bucket that this rewrite has to keep reading.
