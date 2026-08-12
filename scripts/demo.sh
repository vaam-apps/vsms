#!/usr/bin/env bash
# scripts/demo.sh — bring up (or tear down) the full demo chain in one shot:
# a scratch Postgres, sms-gateway, sms-worker (dispatch,scheduler,jobs),
# sms-fake-orange, and the admin console, all wired together with a
# provisioned client. This automates docs/runbooks/getting-started.md
# end to end — read that file for what each step actually does and why.
#
# NOT for production. sms-fake-orange impersonates Orange Cameroon's HTTP
# API; it sends no real SMS to any real handset (see
# app/sms-fake-orange/src/main.rs's own module doc). This script exists so
# a demo or a local smoke test doesn't have to be reassembled by hand every
# time — confirmed end to end (composer send -> delivered, visible on
# /messages) in the session that added it.
#
# Usage:
#   scripts/demo.sh up       # (re)build a fresh demo from scratch
#   scripts/demo.sh down     # stop every process and remove the container
#   scripts/demo.sh status   # what's currently running
#
# Ports (override via env if these collide with something already running):
#   VSMS_DEMO_PG_PORT      postgres,      default 15433
#   VSMS_DEMO_GATEWAY_PORT sms-gateway,   default 8080
#   VSMS_DEMO_ORANGE_PORT  sms-fake-orange, default 8090
#   VSMS_DEMO_CONSOLE_PORT admin console, default 3100 (never 3000 — collides
#                          with common local dev servers on this machine)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PG_CONTAINER="vsms-demo-postgres"
PG_PORT="${VSMS_DEMO_PG_PORT:-15433}"
GATEWAY_PORT="${VSMS_DEMO_GATEWAY_PORT:-8080}"
ORANGE_PORT="${VSMS_DEMO_ORANGE_PORT:-8090}"
CONSOLE_PORT="${VSMS_DEMO_CONSOLE_PORT:-3100}"
DB_NAME="vsms_demo"
DATABASE_URL="postgres://postgres:postgres@localhost:${PG_PORT}/${DB_NAME}"

RUN_DIR="$ROOT/.demo"
PEPPER_FILE="$RUN_DIR/pepper"
KEY_FILE="$RUN_DIR/console-client-key.pem"
ENV_LOCAL="$ROOT/admin/.env.local"
PROCS="fake-orange gateway worker admin"

log() { echo "==> $*"; }

wait_for_postgres() {
  for _ in $(seq 1 30); do
    if docker exec "$PG_CONTAINER" pg_isready -U postgres >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "postgres never became ready in $PG_CONTAINER" >&2
  exit 1
}

start_bg() {
  # start_bg <name> <logfile> <command...> — backgrounds a command, records
  # its pid under $RUN_DIR/<name>.pid, and streams stdout+stderr to a log.
  local name="$1" logfile="$2"
  shift 2
  ("$@" >"$logfile" 2>&1 &
   echo $! > "$RUN_DIR/$name.pid")
}

up() {
  mkdir -p "$RUN_DIR"

  log "starting demo postgres container ($PG_CONTAINER on port $PG_PORT)"
  if docker ps -a --format '{{.Names}}' | grep -qx "$PG_CONTAINER"; then
    docker start "$PG_CONTAINER" >/dev/null
  else
    docker run -d --name "$PG_CONTAINER" -e POSTGRES_PASSWORD=postgres \
      -p "${PG_PORT}:5432" postgres:16 >/dev/null
  fi
  wait_for_postgres

  log "resetting database $DB_NAME (fresh state on every 'up')"
  docker exec "$PG_CONTAINER" psql -U postgres -c "DROP DATABASE IF EXISTS $DB_NAME" >/dev/null
  docker exec "$PG_CONTAINER" psql -U postgres -c "CREATE DATABASE $DB_NAME" >/dev/null

  log "applying migrations"
  DATABASE_URL="$DATABASE_URL" ./ci/apply-migrations.sh >/dev/null

  log "generating a fresh SMS_HASH_PEPPER for this demo run"
  openssl rand -base64 48 >"$PEPPER_FILE"
  local pepper
  pepper="$(cat "$PEPPER_FILE")"

  log "building the workspace's binaries and examples (first run is slow)"
  cargo build -q --workspace --bins --examples

  log "rotating the OP signing key"
  DATABASE_URL="$DATABASE_URL" ./target/debug/sms-gateway rotate-signing-key

  log "seeding fixtures + a first message via send_test_message"
  local seed_out app_id
  seed_out="$(DATABASE_URL="$DATABASE_URL" SMS_HASH_PEPPER="$pepper" \
    ./target/debug/examples/send_test_message \
    --to +237677123456 --sender-id VYMALO --body "Hello from the vsms demo script")"
  echo "$seed_out"
  app_id="$(echo "$seed_out" | sed -n 's/^created App \([a-z0-9]*\).*/\1/p')"
  if [ -z "$app_id" ]; then
    echo "could not parse an App id out of send_test_message's output" >&2
    exit 1
  fi
  # Persisted so a second script (scripts/e2e-integration.sh) can
  # provision an additional client against this same App without
  # reparsing send_test_message's own stdout — this file is this
  # script's own committed contract for that reuse, not incidental.
  echo -n "$app_id" >"$RUN_DIR/app-id"

  log "provisioning a console client for App $app_id"
  rm -f "$KEY_FILE"
  local prov_out client_id
  # job:read/job:enqueue/worker:read (#56/#57): the admin console's Jobs and
  # Workers screens are gated behind these scopes at Layer 2
  # (`require_permission`) precisely because `Job`'s own Layer 1 `@@allow`
  # admits any `auth().kind == "app"` caller unscoped (no `appId` to filter
  # by — see `schema.cstack`'s own comment on `Job`) — a client provisioned
  # without them can authenticate fine but gets a 403 reading the job
  # backlog or the workers screen, same as it already would calling
  # sendMessage without `sms:send`.
  #
  # provider:read/route:read (#54): the identical shape, for the Providers/
  # Routes screens and the route simulator — `Provider`/`Route.read` gained
  # `auth().kind == "app"` in this same PR, so these two scopes are what
  # actually lets this console's credential list either model or call
  # `simulateRoute` (`crates/sms-api/src/procedures.rs::Procedures::simulate`'s
  # own `require_permission(ctx, "route:read")`). Editing either model still
  # needs a human role no token this deployment can issue carries (#194) —
  # scope alone doesn't change that, see `providers-screen.tsx`'s own doc.
  #
  # dashboard:read (#49): the Dashboard screen's own `dashboardSummary`
  # procedure — same shape again, `DashboardSummary` isn't a model, so its
  # `@allow` admits any `auth().kind == "app"` caller unconditionally and
  # this scope is the real perimeter (`require_permission(ctx,
  # "dashboard:read")`, `crates/sms-api/src/procedures.rs`'s own
  # `dashboard_snapshot`).
  prov_out="$(DATABASE_URL="$DATABASE_URL" SMS_HASH_PEPPER="$pepper" \
    ./target/debug/sms-gateway provision-client \
    --app-id "$app_id" --label "demo console" \
    --scope sms:send --scope sms:read \
    --scope job:read --scope job:enqueue --scope worker:read \
    --scope provider:read --scope route:read --scope dashboard:read \
    --key-out "$KEY_FILE")"
  echo "$prov_out"
  client_id="$(echo "$prov_out" | sed -n 's/^provisioned client: \(.*\)$/\1/p')"
  # Same reuse contract as app-id above.
  echo -n "$client_id" >"$RUN_DIR/console-client-id"

  # #194: the console's *human* login is a separate credential path from
  # the machine client above. Two distinct things are needed and neither
  # has a generated-CRUD route that any real token could reach:
  #
  #   1. the `sms-console` OauthClient row, registered with the one exact
  #      redirect_uri authkestra matches literally (RFC 6749 3.1.2), and
  #   2. a real User + Argon2id UserCredential to actually sign in as.
  #
  # Without both, `just demo` comes up with a console nobody can log into
  # — which is exactly what it did between #206 landing and this change.
  log "registering the sms-console OIDC client"
  DATABASE_URL="$DATABASE_URL" ./target/debug/sms-gateway seed-console-client \
    --client-id sms-console \
    --redirect-uri "http://127.0.0.1:${CONSOLE_PORT}/api/auth/callback"

  log "provisioning the demo console operator"
  local user_out
  user_out="$(DATABASE_URL="$DATABASE_URL" ./target/debug/sms-gateway provision-user \
    --email demo@vsms.local --display-name "Demo Operator" --role-key owner)"
  echo "$user_out"
  # Surfaced again in the final summary — a generated password printed a
  # hundred lines up is a password nobody finds. Pull out just the two
  # values a person needs; the command's own prose (rotation caveat,
  # handling advice) already printed in full above.
  DEMO_LOGIN_EMAIL="$(echo "$user_out" | sed -n 's/^provisioned user: \([^ ]*\).*/\1/p')"
  DEMO_LOGIN_PASSWORD="$(echo "$user_out" | sed -n 's/^.*never shown again): \(.*\)$/\1/p')"

  log "starting sms-fake-orange on :$ORANGE_PORT (impersonation only — no real SMS)"
  start_bg fake-orange "$RUN_DIR/fake-orange.log" \
    ./target/debug/sms-fake-orange \
    --bind-addr "127.0.0.1:${ORANGE_PORT}" \
    --dlr-endpoint "http://127.0.0.1:${GATEWAY_PORT}/dlr/orange_cm" \
    --sender-number +2370000

  log "starting sms-gateway on :$GATEWAY_PORT"
  DATABASE_URL="$DATABASE_URL" \
  SMS_HASH_PEPPER="$pepper" \
  SMS_OIDC_ISSUER="http://127.0.0.1:${GATEWAY_PORT}" \
  ORANGE_CM_CLIENT_ID=placeholder \
  ORANGE_CM_CLIENT_SECRET=placeholder \
  ORANGE_CM_SENDER_NUMBER=+2370000 \
  ORANGE_CM_BASE_URL="http://127.0.0.1:${ORANGE_PORT}" \
    start_bg gateway "$RUN_DIR/gateway.log" \
    ./target/debug/sms-gateway serve --listen "127.0.0.1:${GATEWAY_PORT}"

  log "starting sms-worker (dispatch,scheduler,jobs)"
  DATABASE_URL="$DATABASE_URL" \
  SMS_WORKER_ROLES=dispatch,scheduler,jobs \
  ORANGE_CM_CLIENT_ID=placeholder \
  ORANGE_CM_CLIENT_SECRET=placeholder \
  ORANGE_CM_SENDER_NUMBER=+2370000 \
  ORANGE_CM_BASE_URL="http://127.0.0.1:${ORANGE_PORT}" \
  ORANGE_CM_DLR_NOTIFY_URL="http://127.0.0.1:${GATEWAY_PORT}/dlr/orange_cm" \
    start_bg worker "$RUN_DIR/worker.log" ./target/debug/sms-worker

  log "writing admin/.env.local"
  cat >"$ENV_LOCAL" <<EOF
# Written by scripts/demo.sh — regenerated on every 'up', safe to discard.
SMS_API_URL=http://127.0.0.1:${GATEWAY_PORT}

# #194: the human authorization-code + PKCE login flow. DASHBOARD_AUTH is
# gone — a hard cutover, not a parallel path — so these three are required
# rather than optional, and @vsms/env refuses to boot without them.
# ADMIN_BASE_URL must be the literal origin a browser reaches this console
# at, because the redirect_uri is matched exactly (RFC 6749 3.1.2), not by
# prefix. The session secret is demo-only and deliberately obvious.
ADMIN_BASE_URL=http://127.0.0.1:${CONSOLE_PORT}
SMS_CONSOLE_OIDC_CLIENT_ID=sms-console
SMS_CONSOLE_SESSION_SECRET=demo-only-session-secret-not-for-any-real-deployment

SMS_AUTH_ISSUER=http://127.0.0.1:${GATEWAY_PORT}
SMS_CONSOLE_CLIENT_ID=${client_id}
SMS_CONSOLE_PRIVATE_KEY_PATH=${KEY_FILE}
SMS_CONSOLE_SCOPE=sms:send sms:read job:read job:enqueue worker:read provider:read route:read dashboard:read

MESSAGE_STREAM_POLL_MS=2000

NEXT_PUBLIC_APP_NAME=vsms Admin Console (demo)
NODE_ENV=development
EOF

  log "pnpm install (workspace root)"
  pnpm install --silent

  log "starting the admin console on :$CONSOLE_PORT"
  (cd admin && start_bg admin "$RUN_DIR/admin.log" pnpm exec next dev -p "$CONSOLE_PORT")

  echo
  log "demo is up"
  echo "    admin console:  http://127.0.0.1:${CONSOLE_PORT}/"
  echo "    sign in with:   ${DEMO_LOGIN_EMAIL:-demo@vsms.local} / ${DEMO_LOGIN_PASSWORD:-see provision-user output above}"
  echo "    sms-gateway:    http://127.0.0.1:${GATEWAY_PORT}/"
  echo "    sms-fake-orange (NOT a real provider): http://127.0.0.1:${ORANGE_PORT}/"
  echo "    postgres:       $DATABASE_URL"
  echo
  echo "    logs: $RUN_DIR/{fake-orange,gateway,worker,admin}.log"
  echo "    stop everything: scripts/demo.sh down"
}

down() {
  log "stopping demo processes"
  for name in $PROCS; do
    local pidfile="$RUN_DIR/$name.pid"
    [ -f "$pidfile" ] || continue
    local pid
    pid="$(cat "$pidfile")"
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      for _ in $(seq 1 10); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.5
      done
      kill -9 "$pid" 2>/dev/null || true
    fi
    rm -f "$pidfile"
  done

  log "removing demo postgres container ($PG_CONTAINER only — never anything else)"
  docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true

  log "demo stopped"
}

status() {
  for name in $PROCS; do
    local pidfile="$RUN_DIR/$name.pid"
    if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
      echo "$name: running (pid $(cat "$pidfile"))"
    else
      echo "$name: not running"
    fi
  done
  if docker ps --format '{{.Names}}' | grep -qx "$PG_CONTAINER"; then
    echo "postgres: running ($PG_CONTAINER)"
  else
    echo "postgres: not running"
  fi
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  status) status ;;
  *)
    echo "usage: $0 {up|down|status}" >&2
    exit 1
    ;;
esac
