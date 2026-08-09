#!/bin/sh
# Installs BACKUP_CRON_SCHEDULE into busybox crond's root crontab and runs
# crond in the foreground as PID 1 — see backup.Dockerfile's own header
# for why crond over a sleep loop or an extra scheduler daemon.
#
# Cron job output is redirected to /proc/1/fd/1 / /proc/1/fd/2 rather than
# left to crond's own default (mail, which nothing in this container
# consumes): PID 1 in THIS container is crond itself, so those paths are
# always this container's own stdout/stderr — visible to
# `docker compose logs backup`, not silently dropped.
set -eu

: "${BACKUP_CRON_SCHEDULE:=0 3 * * *}"

echo "${BACKUP_CRON_SCHEDULE} /backup.sh >>/proc/1/fd/1 2>>/proc/1/fd/2" >/etc/crontabs/root
echo "vsms-backup: schedule = ${BACKUP_CRON_SCHEDULE}"

if [ "${BACKUP_RUN_ON_START:-true}" = "true" ]; then
  echo "vsms-backup: BACKUP_RUN_ON_START=true — running an initial backup before the first scheduled tick"
  if ! /backup.sh; then
    echo "vsms-backup: initial backup failed — will retry on the next scheduled run (${BACKUP_CRON_SCHEDULE})" >&2
  fi
fi

exec crond -f -l 2
