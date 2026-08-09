#!/usr/bin/env bash
# scripts/e2e-integration.sh — proves #160: the two halves of the
# integration story (`just demo`'s composer-to-delivered run, and
# `examples/rust/sms-send`'s third-party-backend-to-delivered run) joined
# into one observation: an external client sends a message over real HTTP,
# and the SAME message id is then read back through the admin console's
# own data path, authenticated as the console's own principal.
#
# What this proves, step by step, all over real HTTP with no in-process
# shortcuts:
#   1. scripts/demo.sh brings up the whole stack (reused, not
#      reimplemented) and provisions ONE App with a "demo console" client.
#   2. A SECOND, independent AppClient ("external integrator") is
#      provisioned against that SAME App — see "Why the same App" below
#      for why this is the correct, not a fudged, choice.
#   3. examples/rust/sms-send authenticates as the integrator (its own
#      private_key_jwt exchange, its own access token) and calls
#      `POST /$procs/sendMessage` — a genuinely separate process, a
#      genuinely separate credential, never touching Procedures directly.
#   4. This script mints a THIRD, independent access token — the console's
#      own credential, read from admin/.env.local, the exact identity
#      admin's Next.js server holds — and polls `GET /messages/{id}`
#      (the same route `packages/gateway/src/messages.ts`'s
#      `getMessageById` calls) until that exact id reaches `delivered`.
#      A 404 here would mean "exists, but not visible to this principal"
#      (`getMessageById`'s own doc) — this script treats that as a
#      reportable finding, never as something to route around.
#
# Why the same App (not two): `Message`'s own row policy in
# schema/schema.cstack is `auth().kind == "user" || appId == auth().appId
# || hasRole('system')`. No human-login role exists yet (AGENTS.md's M1
# section — GatewayAuth only ever mints role="app"/"system"), so the
# console's credential is itself just another App-scoped principal, not
# a cross-tenant "operator" one — admin/app/messages/messages-screen.tsx
# says exactly this in its own on-screen banner ("Scoped to this app
# only... This is not a filter and not a bug"). Provisioning the
# integrator under a DIFFERENT App would prove nothing this deployment
# claims to support today; it would just rediscover that documented
# scope cut. Provisioning both under the SAME App is what actually
# matches the product story here: a tenant's own console access and a
# tenant's own backend integration are two separate credentials
# (different clientId, different private key, different scopes) that
# both legitimately act on behalf of that one tenant. See
# docs/runbooks/e2e-integration.md for the full reasoning and for what
# this script does NOT prove (cross-tenant visibility; real Orange
# delivery — sms-fake-orange is a fake, per #36).
#
# Usage: scripts/e2e-integration.sh [--to <msisdn>]
# Exits non-zero and prints the exact failing step if any link breaks —
# never silently degrades into a weaker assertion.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

GATEWAY_PORT="${VSMS_DEMO_GATEWAY_PORT:-8080}"
ISSUER="http://127.0.0.1:${GATEWAY_PORT}"
RUN_DIR="$ROOT/.demo"
ENV_LOCAL="$ROOT/admin/.env.local"
INTEGRATOR_KEY="$RUN_DIR/integrator-client-key.pem"
EXAMPLE_MANIFEST="$ROOT/examples/rust/Cargo.toml"
EXAMPLE_BIN="$ROOT/examples/rust/target/debug/vsms-example-send"

TO_MSISDN="+237677000222"
while [ $# -gt 0 ]; do
  case "$1" in
    --to)
      TO_MSISDN="$2"
      shift 2
      ;;
    *)
      echo "usage: $0 [--to <msisdn>]" >&2
      exit 1
      ;;
  esac
done

log() { echo "==> $*"; }
fail() {
  echo "FAILED: $*" >&2
  exit 1
}

require_file() {
  [ -f "$1" ] || fail "expected file missing: $1 (did scripts/demo.sh up run to completion?)"
}

# base64url, no padding — RFC 7515 §2. `openssl base64 -A` emits standard
# base64 on one line; translate to the URL-safe alphabet and strip `=`.
b64url() {
  openssl base64 -A | tr '+/' '-_' | tr -d '='
}

# mint_client_jwt <client_id> <key_path> — a fresh RFC 7523 §3
# private_key_jwt client assertion, hand-signed with the caller's own RSA
# key via `openssl dgst -sign` (no extra JWT dependency: this script is
# bash-only on purpose, matching this repo's stated shell-script
# convention). Mirrors packages/gateway/src/token.ts's own mintAssertion
# and examples/rust/sms-send's own sign_assertion field-for-field: iss=sub
# =client_id, aud=token endpoint, a fresh jti every call (ClientAssertion
# is insert-only and replay-protects on it), 60s TTL.
mint_client_assertion() {
  local client_id="$1" key_path="$2"
  local now exp jti header payload signing_input signature
  now="$(date +%s)"
  exp="$((now + 60))"
  jti="$(uuidgen | tr '[:upper:]' '[:lower:]')"

  header="$(printf '{"alg":"RS256","typ":"JWT","kid":"%s"}' "$client_id" | b64url)"
  payload="$(
    printf '{"iss":"%s","sub":"%s","aud":"%s/token","jti":"%s","iat":%s,"exp":%s}' \
      "$client_id" "$client_id" "$ISSUER" "$jti" "$now" "$exp" | b64url
  )"
  signing_input="${header}.${payload}"
  signature="$(printf '%s' "$signing_input" | openssl dgst -sha256 -sign "$key_path" | b64url)"
  printf '%s.%s' "$signing_input" "$signature"
}

# mint_access_token <client_id> <key_path> <scope> — the real
# private_key_jwt exchange at POST {issuer}/token, over real HTTP.
mint_access_token() {
  local client_id="$1" key_path="$2" scope="$3"
  local assertion response
  assertion="$(mint_client_assertion "$client_id" "$key_path")"
  response="$(
    curl -sS -w '\n%{http_code}' -X POST "${ISSUER}/token" \
      --data-urlencode "grant_type=client_credentials" \
      --data-urlencode "client_id=${client_id}" \
      --data-urlencode "client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer" \
      --data-urlencode "client_assertion=${assertion}" \
      --data-urlencode "scope=${scope}"
  )"
  local status body
  status="$(echo "$response" | tail -n1)"
  body="$(echo "$response" | sed '$d')"
  [ "$status" = "200" ] || fail "token exchange for $client_id failed ($status): $body"
  echo "$body" | jq -r '.access_token'
}

log "1/6 bringing up the demo stack (scripts/demo.sh — resets to a fresh state every time)"
# `demo.sh up` alone refuses to DROP DATABASE while a previous run's own
# gateway/worker/admin processes are still holding connections open
# ("database is being accessed by other users") — found running this
# script twice in a row without a manual `demo.sh down` in between. A
# `down` first (harmless, and a no-op if nothing from a prior run is
# left) is what makes THIS script the single, always-safely-rerunnable
# command #160 asks for, rather than something that only works once per
# terminal session.
"$ROOT/scripts/demo.sh" down || true
"$ROOT/scripts/demo.sh" up

require_file "$RUN_DIR/app-id"
require_file "$RUN_DIR/console-client-id"
require_file "$ENV_LOCAL"
APP_ID="$(cat "$RUN_DIR/app-id")"
CONSOLE_CLIENT_ID="$(cat "$RUN_DIR/console-client-id")"
CONSOLE_KEY="$(grep '^SMS_CONSOLE_PRIVATE_KEY_PATH=' "$ENV_LOCAL" | cut -d= -f2-)"
require_file "$CONSOLE_KEY"
log "    App: $APP_ID   console client: $CONSOLE_CLIENT_ID"

log "2/6 provisioning a SECOND, independent client for the same App — the external integrator"
rm -f "$INTEGRATOR_KEY"
PROV_OUT="$(
  DATABASE_URL="postgres://postgres:postgres@localhost:${VSMS_DEMO_PG_PORT:-15433}/vsms_demo" \
  SMS_HASH_PEPPER="$(cat "$RUN_DIR/pepper")" \
    "$ROOT/target/debug/sms-gateway" provision-client \
    --app-id "$APP_ID" --label "external integrator (e2e-integration)" \
    --scope sms:send --scope sms:read --key-out "$INTEGRATOR_KEY"
)"
echo "$PROV_OUT"
INTEGRATOR_CLIENT_ID="$(echo "$PROV_OUT" | sed -n 's/^provisioned client: \(.*\)$/\1/p')"
[ -n "$INTEGRATOR_CLIENT_ID" ] || fail "could not parse the integrator client id out of provision-client's output"
[ "$INTEGRATOR_CLIENT_ID" != "$CONSOLE_CLIENT_ID" ] || fail "integrator client id collided with the console's — not two separate principals"
log "    integrator client: $INTEGRATOR_CLIENT_ID (same App, different credential — the point of #160)"

log "3/6 building examples/rust/sms-send (a genuinely separate Cargo workspace)"
cargo build -q --manifest-path "$EXAMPLE_MANIFEST" -p vsms-example-send

CLIENT_REF="e2e-$(date +%s)-$(uuidgen | tr '[:upper:]' '[:lower:]' | cut -c1-8)"
log "4/6 sending as the integrator, over real HTTP (clientRef=$CLIENT_REF)"
SEND_OUT="$(
  "$EXAMPLE_BIN" \
    --issuer "$ISSUER" \
    --client-id "$INTEGRATOR_CLIENT_ID" \
    --private-key-path "$INTEGRATOR_KEY" \
    --to "$TO_MSISDN" \
    --sender-id VYMALO \
    --body "Hello from the vsms e2e-integration scenario (#160)" \
    --client-ref "$CLIENT_REF"
)"
echo "$SEND_OUT"
MESSAGE_ID="$(echo "$SEND_OUT" | sed -n 's/^sent: messageId=\([a-z0-9]*\) .*/\1/p')"
[ -n "$MESSAGE_ID" ] || fail "could not parse a messageId out of vsms-example-send's output"
log "    message id: $MESSAGE_ID"

log "5/6 minting the console's OWN access token (same identity admin's Next.js server holds)"
CONSOLE_TOKEN="$(mint_access_token "$CONSOLE_CLIENT_ID" "$CONSOLE_KEY" "sms:read sms:send")"
[ -n "$CONSOLE_TOKEN" ] && [ "$CONSOLE_TOKEN" != "null" ] || fail "console token exchange returned no access_token"

log "6/6 polling GET /messages/{id} AS THE CONSOLE — the exact route packages/gateway/src/messages.ts's getMessageById calls — until delivered"
DEADLINE=$(($(date +%s) + 60))
LAST_STATE=""
STATES_SEEN=""
FOUND_DELIVERED=0
while [ "$(date +%s)" -lt "$DEADLINE" ]; do
  RESPONSE="$(
    curl -sS -w '\n%{http_code}' \
      -H "Authorization: Bearer ${CONSOLE_TOKEN}" \
      -H "Accept: application/json" \
      "${ISSUER}/messages/${MESSAGE_ID}"
  )"
  HTTP_STATUS="$(echo "$RESPONSE" | tail -n1)"
  HTTP_BODY="$(echo "$RESPONSE" | sed '$d')"

  if [ "$HTTP_STATUS" = "404" ]; then
    fail "GET /messages/${MESSAGE_ID} returned 404 under the CONSOLE's own credential. Per \
packages/gateway/src/messages.ts's own module doc (point 9), sms-api cannot distinguish \
\"never existed\" from \"exists but belongs to another App\" — this means the console's \
principal (App $APP_ID) cannot see a message that unquestionably exists (it was just sent \
and read back successfully under the integrator's own credential). THIS IS A FINDING, not a \
bug to route around: report it, do not retry past it."
  fi
  [ "$HTTP_STATUS" = "200" ] || fail "GET /messages/${MESSAGE_ID} failed ($HTTP_STATUS): $HTTP_BODY"

  STATE="$(echo "$HTTP_BODY" | jq -r '.state')"
  RETURNED_APP_ID="$(echo "$HTTP_BODY" | jq -r '.appId')"
  [ "$RETURNED_APP_ID" = "$APP_ID" ] || fail "GET /messages/${MESSAGE_ID} returned appId=$RETURNED_APP_ID, expected $APP_ID"

  if [ "$STATE" != "$LAST_STATE" ]; then
    echo "    [$(date +%H:%M:%S)] state=$STATE"
    STATES_SEEN="${STATES_SEEN}${STATE} "
    LAST_STATE="$STATE"
  fi

  if [ "$STATE" = "delivered" ]; then
    FOUND_DELIVERED=1
    break
  fi
  if [ "$STATE" = "failed" ] || [ "$STATE" = "rejected" ] || [ "$STATE" = "expired" ] || [ "$STATE" = "undelivered" ]; then
    fail "message $MESSAGE_ID reached a terminal non-delivered state: $STATE (full row: $HTTP_BODY)"
  fi

  sleep 1
done

[ "$FOUND_DELIVERED" = "1" ] || fail "message $MESSAGE_ID did not reach delivered within 60s (last state: $LAST_STATE)"

echo
log "PASSED"
echo "    App:                 $APP_ID"
echo "    console client:      $CONSOLE_CLIENT_ID"
echo "    integrator client:   $INTEGRATOR_CLIENT_ID"
echo "    message id:          $MESSAGE_ID"
echo "    clientRef:           $CLIENT_REF"
echo "    state progression:   $STATES_SEEN"
echo
echo "    Verify in a real browser: http://127.0.0.1:${VSMS_DEMO_CONSOLE_PORT:-3100}/messages?clientRef=${CLIENT_REF}"
echo "    (Orange is FAKED end to end here — sms-fake-orange, not a real carrier. See #36.)"
