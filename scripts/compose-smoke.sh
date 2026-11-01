#!/usr/bin/env bash
# Clean-host smoke path for feature 004 (FR-023, SC-009, SC-011).
#
# Starts the shipped compose stack, walks the operator -> administrator -> project
# -> ingest -> raw -> retention -> restart path, re-applies the compatibility
# migration to prove idempotency, and writes a dated evidence file.
#
# Usage:
#   ./scripts/compose-smoke.sh                # full path
#   ./scripts/compose-smoke.sh --compat-only  # only the compatibility checks
#   ./scripts/compose-smoke.sh --skip-up      # assume the stack is already running
#
# Requires: bash, curl, jq, docker compose, and (for the retention step) psql
# inside the postgres container.

set -euo pipefail

BASE_URL="${SCOUT_URL:-http://127.0.0.1:8080}"
OPERATOR_EMAIL="${SCOUT_OPERATOR_EMAIL:-operator@scout.local}"
OPERATOR_PASSWORD="${SCOUT_OPERATOR_PASSWORD:-change-me-now}"
EVIDENCE_DIR="${SCOUT_EVIDENCE_DIR:-specs/004-production-readiness/evidence}"
COMPAT_ONLY=0
SKIP_UP=0

usage() {
  sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --compat-only) COMPAT_ONLY=1 ;;
    --skip-up) SKIP_UP=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 2; }
done

STAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
DATE="$(date -u +%F)"
EVIDENCE="$EVIDENCE_DIR/compose-smoke-$DATE.md"
mkdir -p "$EVIDENCE_DIR"
RESULTS=()
STATUS="FAIL"

record() { RESULTS+=("$1"); printf '  ok  %s\n' "$1"; }
fail() {
  RESULTS+=("FAILED: $1")
  printf '  FAIL %s\n' "$1" >&2
  STATUS="FAIL"
  write_evidence
  exit 1
}

write_evidence() {
  local result="$1"
  {
    echo "# Compose smoke — $DATE"
    echo
    echo "- Commit: $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    echo "- Host: $(uname -srm)"
    echo "- Base URL: $BASE_URL"
    echo "- Started: $STAMP"
    echo "- Command: $0"
    echo "- Result: $result"
    echo
    echo "## Steps"
    echo
    if [ "${#RESULTS[@]}" -gt 0 ]; then
      for line in "${RESULTS[@]}"; do echo "- $line"; done
    else
      echo "- (no steps recorded)"
    fi
  } >"$EVIDENCE"
}
trap 'write_evidence "$STATUS"' EXIT

api() { # method path [token] [json-body]
  local method="$1" path="$2" token="${3:-}" body="${4:-}"
  local args=(-sS -X "$method" -H 'Content-Type: application/json')
  if [ -n "$token" ]; then args+=(-H "Authorization: Bearer $token"); fi
  if [ -n "$body" ]; then args+=(-d "$body"); fi
  curl "${args[@]}" "$BASE_URL$path"
}

status_for() { curl -s -o /dev/null -w '%{http_code}' "$@"; }

wait_health() {
  local tries="${1:-60}" i code
  for i in $(seq 1 "$tries"); do
    code=$(curl -s -o /tmp/scout-health.json -w '%{http_code}' "$BASE_URL/api/health" || true)
    if [ "$code" = "200" ]; then return 0; fi
    sleep 2
  done
  return 1
}

hex32() { openssl rand -hex 16 2>/dev/null || date +%s%N; }

envelope_body() { # event-id
  printf '%s\n%s\n%s\n' \
    "{\"event_id\":\"$1\"}" \
    '{"type":"event"}' \
    "{\"event_id\":\"$1\",\"timestamp\":$(date +%s),\"platform\":\"other\",\"level\":\"error\",\"message\":\"compose smoke\",\"exception\":{\"values\":[{\"type\":\"SmokeError\",\"value\":\"compose smoke\"}]}}"
}

compose_up() {
  docker compose up -d --build --force-recreate || fail "docker compose up failed"
  record "compose stack started with --build --force-recreate"
}

wait_for_health() {
  wait_health 60 || fail "the stack did not become healthy within 120s"
  local body
  body="$(cat /tmp/scout-health.json)"
  [ "$(jq -r '.status' <<<"$body")" = "ok" ] || fail "health is not ok: $body"
  record "health is ok: $(jq -c '{storage, store, durability, object_store}' <<<"$body")"
}

operator_session() {
  local body
  body="$(api POST /api/login '' "{\"email\":\"$OPERATOR_EMAIL\",\"password\":\"$OPERATOR_PASSWORD\"}")" \
    || fail "operator sign-in request failed"
  OPERATOR_TOKEN="$(jq -r '.token // empty' <<<"$body")"
  [ -n "$OPERATOR_TOKEN" ] || fail "operator sign-in failed: $body"
  record "operator signed in ($OPERATOR_EMAIL)"
}

provision() {
  local org_body org_id setup_token admin_token project_body key_body
  org_body="$(api POST /api/orgs "$OPERATOR_TOKEN" \
    "{\"name\":\"Smoke $(date +%s)\",\"initial_admin_email\":\"smoke-admin@example.test\"}")"
  org_id="$(jq -r '.organisation.id // empty' <<<"$org_body")"
  setup_token="$(jq -r '.initial_admin_setup_token // empty' <<<"$org_body")"
  [ -n "$org_id" ] && [ -n "$setup_token" ] || fail "organisation creation failed: $org_body"
  record "operator created organisation $org_id with a one-time setup token"

  admin_token="$(api POST "/api/invitations/$setup_token/accept" '' \
    '{"name":"Smoke Admin","password":"smoke-admin-password"}' | jq -r '.token // empty')"
  [ -n "$admin_token" ] || fail "initial-admin setup token redemption failed"
  record "initial admin redeemed the setup token"

  # FR-034: only the operator creates projects, and it must name an existing
  # organisation member as the initial owner.
  project_body="$(api POST "/api/orgs/$org_id/projects" "$OPERATOR_TOKEN" \
    '{"name":"smoke","slug":"smoke","initial_owner_email":"smoke-admin@example.test"}')"
  PROJECT_ID="$(jq -r '.project.id // empty' <<<"$project_body")"
  DSN_ID="$(jq -r '.project.dsn_id // empty' <<<"$project_body")"
  [ -n "$PROJECT_ID" ] && [ -n "$DSN_ID" ] \
    || fail "project creation failed: $project_body"
  # FR-012: the operator must never receive a credential.
  [ -z "$(jq -r '.key // empty' <<<"$project_body")" ] \
    || fail "project creation returned an ingest key to the operator"
  [ -z "$(jq -r '.dsn // empty' <<<"$project_body")" ] \
    || fail "project creation returned a DSN to the operator"
  record "operator created project $PROJECT_ID with DSN id $DSN_ID and received no credential"

  # The named owner creates the first key and is the only holder of it.
  key_body="$(api POST "/api/projects/$PROJECT_ID/keys" "$admin_token" '{"label":"Smoke"}')"
  INGEST_KEY="$(jq -r '.key // empty' <<<"$key_body")"
  [ -n "$INGEST_KEY" ] || fail "the initial owner could not create an ingest key: $key_body"
  ADMIN_TOKEN="$admin_token"
  record "initial owner created the first ingest key and holds its DSN"
}

ingest_and_read() {
  local event_id code body issue_id
  event_id="$(hex32)"
  envelope_body "$event_id" >/tmp/scout-envelope.txt
  code="$(status_for -X POST "$BASE_URL/api/$DSN_ID/envelope" \
    -H "X-Scout-Key: $INGEST_KEY" \
    -H 'Content-Type: application/x-sentry-envelope' \
    --data-binary @/tmp/scout-envelope.txt)"
  [ "$code" = "200" ] || fail "event ingest returned $code"
  record "event ingested into project $PROJECT_ID"

  code="$(status_for -X POST "$BASE_URL/api/$DSN_ID/logs" \
    -H "X-Scout-Key: $INGEST_KEY" \
    -H 'Content-Type: application/json' \
    -d '{"message":"compose smoke log","level":"info"}')"
  [ "$code" = "200" ] || fail "log ingest returned $code"
  record "log line ingested into project $PROJECT_ID"

  issue_id=""
  for _ in $(seq 1 20); do
    body="$(api GET "/api/projects/$PROJECT_ID/issues" "$ADMIN_TOKEN")"
    issue_id="$(jq -r '.issues[0].id // empty' <<<"$body")"
    if [ -n "$issue_id" ]; then break; fi
    sleep 1
  done
  [ -n "$issue_id" ] || fail "the ingested event never became visible as an issue"
  ISSUE_ID="$issue_id"
  record "issue $issue_id is visible through the API"

  code="$(status_for -H "Authorization: Bearer $ADMIN_TOKEN" "$BASE_URL/api/issues/$issue_id/raw")"
  [ "$code" = "200" ] || fail "raw payload retrieval returned $code"
  record "raw payload is retrievable"
}

retention_and_restart() {
  local code body
  docker compose exec -T postgres psql -U scout -d scout -q -c \
    "UPDATE events SET received_at = now() - interval '40 days';" >/dev/null \
    || fail "could not backdate events for the retention check"
  record "event rows aged beyond the 30-day retention window"

  SCOUT_RETENTION_INTERVAL_SECONDS=2 docker compose up -d scout >/dev/null \
    || fail "could not restart scout with a short retention interval"
  wait_health 60 || fail "the stack did not become healthy after the retention restart"
  sleep 6

  code="$(status_for -H "Authorization: Bearer $ADMIN_TOKEN" "$BASE_URL/api/issues/$ISSUE_ID/raw")"
  [ "$code" = "404" ] || fail "expired raw payload returned $code, expected 404"
  record "raw payload reports the expired state after retention"

  body="$(api GET "/api/issues/$ISSUE_ID" "$ADMIN_TOKEN")"
  [ "$(jq -r '.issue.id // empty' <<<"$body")" = "$ISSUE_ID" ] \
    || fail "the issue disappeared after retention: $body"
  record "issue and triage history survived retention"

  docker compose restart scout >/dev/null || fail "could not restart scout"
  wait_health 60 || fail "the stack did not become healthy after the restart"
  body="$(api GET "/api/issues/$ISSUE_ID" "$ADMIN_TOKEN")"
  [ "$(jq -r '.issue.id // empty' <<<"$body")" = "$ISSUE_ID" ] \
    || fail "the issue disappeared after a restart: $body"
  record "records survived an application restart"
}

compat_migration() {
  docker compose exec -T postgres psql -U scout -d scout -v ON_ERROR_STOP=1 -q \
    <migrations/0007_sentry_compat.sql >/dev/null || fail "first 0007 application failed"
  docker compose exec -T postgres psql -U scout -d scout -v ON_ERROR_STOP=1 -q \
    <migrations/0007_sentry_compat.sql >/dev/null || fail "second 0007 application failed"
  record "0007_sentry_compat.sql applied twice without error (idempotent)"

  local indexes
  indexes="$(docker compose exec -T postgres psql -U scout -d scout -tAc \
    "SELECT count(*) FROM pg_indexes WHERE indexname IN ('organisations_slug_key','issues_project_number_key')")"
  [ "$indexes" = "2" ] || fail "compatibility unique indexes are missing (found $indexes)"
  record "organisation slug and per-project issue-number uniqueness enforced"

  local code
  code="$(status_for -H "Authorization: Bearer $OPERATOR_TOKEN" "$BASE_URL/api/0/projects/")"
  case "$code" in
    200|401|403|404) record "compatibility surface answered with documented status $code" ;;
    *) fail "compatibility surface returned $code" ;;
  esac
}

main() {
  if [ "$COMPAT_ONLY" = "1" ]; then
    operator_session
    compat_migration
    STATUS="PASS"
    return
  fi

  if [ "$SKIP_UP" = "0" ]; then compose_up; else record "reused the running stack"; fi
  wait_for_health
  operator_session
  provision
  ingest_and_read
  retention_and_restart
  compat_migration
  STATUS="PASS"
  record "all smoke checks passed"
}

main
