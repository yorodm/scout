#!/usr/bin/env bash
# Repeatable ingest load check for feature 004 (FR-024, SC-010).
#
# Sends 10 KB envelopes at a target rate, records accepted throughput, p95
# acknowledgement latency, and acceptance-to-visibility time, then writes a dated
# evidence file. It is a release-evidence tool, not a per-change CI gate.
#
# Usage:
#   SCOUT_DSN_ID=1 SCOUT_INGEST_KEY=scout_... ./scripts/load-ingest.sh \
#       [--rate 500] [--duration 60] [--size 10240] [--concurrency 32]
#
# Optional (enables the visibility measurement):
#   SCOUT_PROJECT_ID=<uuid> SCOUT_API_TOKEN=<dashboard session>
#
# Requires: bash, curl, awk.

set -euo pipefail

BASE_URL="${SCOUT_URL:-http://127.0.0.1:8080}"
DSN_ID="${SCOUT_DSN_ID:-}"
INGEST_KEY="${SCOUT_INGEST_KEY:-}"
PROJECT_ID="${SCOUT_PROJECT_ID:-}"
API_TOKEN="${SCOUT_API_TOKEN:-}"
EVIDENCE_DIR="${SCOUT_EVIDENCE_DIR:-specs/004-production-readiness/evidence}"

RATE=500
DURATION=60
SIZE=10240
CONCURRENCY=32

usage() {
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    --rate) RATE="$2"; shift ;;
    --duration) DURATION="$2"; shift ;;
    --size) SIZE="$2"; shift ;;
    --concurrency) CONCURRENCY="$2"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$DSN_ID" ] || [ -z "$INGEST_KEY" ]; then
  echo "SCOUT_DSN_ID and SCOUT_INGEST_KEY are required" >&2
  exit 2
fi
for tool in curl awk; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 2; }
done

DATE="$(date -u +%F)"
EVIDENCE="$EVIDENCE_DIR/load-check-$DATE.md"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$EVIDENCE_DIR"

hex32() { openssl rand -hex 16 2>/dev/null || date +%s%N; }

# One envelope padded to approximately SIZE bytes.
make_envelope() { # event-id file
  local event_id="$1" file="$2" head tail pad
  head="$(printf '{"event_id":"%s","timestamp":%s,"platform":"other","level":"error","message":"load check %s","padding":"' \
    "$event_id" "$(date +%s)" "$event_id")"
  tail='"}'
  pad=$(( SIZE - ${#head} - 3 ))
  if [ "$pad" -lt 16 ]; then pad=16; fi
  {
    printf '{"event_id":"%s"}\n' "$event_id"
    printf '{"type":"event"}\n'
    printf '%s' "$head"
    awk -v n="$pad" 'BEGIN { while (n-- > 0) printf "x" }'
    printf '%s\n' "$tail"
  } >"$file"
}

# Single request: writes "<status> <seconds>" to the results file.
send_one() { # index results-file
  local index="$1" results="$2" event_id file
  event_id="$(hex32)$(printf '%04x' "$index")"
  file="$WORK/env-$index.txt"
  make_envelope "$event_id" "$file"
  curl -s -o /dev/null \
    -w "%{http_code} %{time_total}\n" \
    -X POST "$BASE_URL/api/$DSN_ID/envelope" \
    -H "X-Scout-Key: $INGEST_KEY" \
    -H 'Content-Type: application/x-sentry-envelope' \
    --data-binary "@$file" >>"$results"
}
export -f send_one make_envelope hex32
export BASE_URL DSN_ID INGEST_KEY SIZE WORK

RESULTS="$WORK/results.txt"
: >"$RESULTS"

TOTAL=$(( RATE * DURATION ))
echo "Sending $TOTAL events at ~$RATE/s ($SIZE bytes each, concurrency $CONCURRENCY) to $BASE_URL ..."
START_NS="$(date +%s%N)"

index=0
while [ "$index" -lt "$TOTAL" ]; do
  batch_end=$(( index + CONCURRENCY ))
  [ "$batch_end" -gt "$TOTAL" ] && batch_end="$TOTAL"
  i="$index"
  while [ "$i" -lt "$batch_end" ]; do
    send_one "$i" "$RESULTS" &
    i=$(( i + 1 ))
  done
  wait
  # Pace the batches so the target rate is respected.
  elapsed_ms=$(( ($(date +%s%N) - START_NS) / 1000000 ))
  target_ms=$(( batch_end * 1000 / RATE ))
  if [ "$target_ms" -gt "$elapsed_ms" ]; then
    sleep_ms=$(( target_ms - elapsed_ms ))
    sleep "$(awk -v ms="$sleep_ms" 'BEGIN { printf "%.3f", ms / 1000 }')"
  fi
  index="$batch_end"
done

END_NS="$(date +%s%N)"
WALL_SECS="$(awk -v ns=$(( END_NS - START_NS )) 'BEGIN { printf "%.2f", ns / 1000000000 }')"

ACCEPTED="$(awk '$1 == 200 { n++ } END { print n + 0 }' "$RESULTS")"
REJECTED=$(( TOTAL - ACCEPTED ))
RATE_ACTUAL="$(awk -v n="$ACCEPTED" -v s="$WALL_SECS" 'BEGIN { if (s > 0) printf "%.1f", n / s; else print "0" }')"

P95="$(awk '{ print $2 }' "$RESULTS" | sort -n | awk '
  { values[NR] = $1 }
  END {
    if (NR == 0) { print "0"; exit }
    idx = int((NR - 1) * 0.95) + 1
    printf "%.0f", values[idx] * 1000
  }')"

VISIBILITY_MS="skipped"
if [ -n "$PROJECT_ID" ] && [ -n "$API_TOKEN" ] && command -v jq >/dev/null 2>&1; then
  VIS_START_NS="$(date +%s%N)"
  for _ in $(seq 1 30); do
    HITS="$(curl -sS -H "Authorization: Bearer $API_TOKEN" \
      "$BASE_URL/api/projects/$PROJECT_ID/issues?q=load" | jq -r '.issues | length' 2>/dev/null || echo 0)"
    if [ "${HITS:-0}" -gt 0 ]; then
      VISIBILITY_MS="$(awk -v ns=$(( $(date +%s%N) - VIS_START_NS )) 'BEGIN { printf "%.0f", ns / 1000000 }')"
      break
    fi
    sleep 1
  done
fi

PASS="yes"
if [ "$(awk -v r="$RATE_ACTUAL" 'BEGIN { print (r >= 500) ? 1 : 0 }')" != "1" ]; then PASS="no"; fi
if [ "$P95" -gt 150 ]; then PASS="no"; fi
if [ "$VISIBILITY_MS" != "skipped" ] && [ "$VISIBILITY_MS" -gt 10000 ]; then PASS="no"; fi

{
  echo "# Load check — $DATE"
  echo
  echo "- Commit: $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
  echo "- Host: $(uname -srm)"
  echo "- Command: $0 --rate $RATE --duration $DURATION --size $SIZE --concurrency $CONCURRENCY"
  echo "- Result: $([ "$PASS" = "yes" ] && echo PASS || echo 'PASS WITH RECORDED SHORTFALL')"
  echo
  echo "## Measurements"
  echo
  echo "| Metric | Target | Measured |"
  echo "|---|---|---|"
  echo "| Accepted events/s | >= 500 | $RATE_ACTUAL |"
  echo "| p95 acknowledgement | <= 150 ms | ${P95} ms |"
  echo "| Time to visibility | <= 10 s | ${VISIBILITY_MS} ms |"
  echo "| Accepted / attempted | — | $ACCEPTED / $TOTAL |"
  echo "| Rejected | — | $REJECTED |"
  echo "| Wall clock | — | ${WALL_SECS}s |"
  echo
  echo "## Notes"
  echo
  echo "- Targets come from SC-010 and the constitution's performance constraint."
  echo "- A shortfall is only acceptable with its host profile recorded above and an explicit acceptance note."
  echo "- Visibility is measured only when SCOUT_PROJECT_ID and SCOUT_API_TOKEN are provided."
} >"$EVIDENCE"

echo
echo "Accepted: $ACCEPTED / $TOTAL (${RATE_ACTUAL}/s), p95 ${P95} ms, visibility ${VISIBILITY_MS} ms"
echo "Evidence: $EVIDENCE"
