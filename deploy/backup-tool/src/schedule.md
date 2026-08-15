`schedule` — this container's own entrypoint when run unattended
(`deploy/backup.Dockerfile`'s `ENTRYPOINT`). Replaces the old
`deploy/backup-entrypoint.sh` + Alpine's busybox `crond` entirely, not
just the shell wrapper around it: `cron` parses `BACKUP_CRON_SCHEDULE`
and computes the next occurrence in-process, so there is no
`/etc/crontabs/root` file to write, no `/proc/1/fd/{1,2}` redirection
trick to route crond's own mail-by-default output back to this
container's stdout/stderr (this binary, being PID 1 itself, already
*is* that stdout/stderr — nothing to redirect), and no second process
for this container to be liveness-checked through.

One correctness property the old shell entrypoint got for free by
`exec`ing into `crond` and never had to think about: **this container
is PID 1 inside its own PID namespace**, and the Linux kernel exempts
PID 1 from a signal's *default* disposition unless the process has
explicitly installed a handler for it — so an unhandled `SIGTERM`
here would be silently ignored, not "terminate like every other
process," and `docker stop`/`docker compose down` would hang for the
full stop-grace-period before falling back to `SIGKILL`, every time.
`signal_hook` below is what makes this exit promptly instead.
