#!/bin/bash
#
# Test astrometry.net plate solving via the JSON API.
#
# Usage: ./test_submission.sh <image_file> <host:port>   (the nova API lives at /nova)
# Example: ./test_submission.sh ./solver/sample.png localhost:7222

set -euo pipefail

FILE="${1:-}"
SERVER="${2:-}"
API_KEY="1jcrmadfnxngxscd"
POLL_INTERVAL=5
TIMEOUT=600  # give up after 10 minutes

if [ -z "$FILE" ] || [ -z "$SERVER" ]; then
    echo "Usage: $0 <image_file> <host:port>"
    echo "Example: $0 ./solver/sample.png localhost:7222"
    exit 1
fi

if [ ! -f "$FILE" ]; then
    echo "Error: file '${FILE}' not found"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    echo "Error: jq is required (sudo pacman -S jq)"
    exit 1
fi

BASE_URL="http://${SERVER}/nova"
SECONDS=0

# ── Login ────────────────────────────────────────────────────────────
echo "--- Login ---"
LOGIN_RESP=$(curl -s -X POST "${BASE_URL}/api/login" \
    -F "request-json={\"apikey\": \"${API_KEY}\"}")

STATUS=$(echo "$LOGIN_RESP" | jq -r '.status')
if [ "$STATUS" != "success" ]; then
    echo "Login failed:"
    echo "$LOGIN_RESP" | jq .
    exit 1
fi

SESSION=$(echo "$LOGIN_RESP" | jq -r '.session')
echo "Authenticated. Session: ${SESSION}"

# ── Upload ───────────────────────────────────────────────────────────
echo ""
echo "--- Upload: $(basename "$FILE") ---"
UPLOAD_RESP=$(curl -s -X POST "${BASE_URL}/api/upload" \
    -F "request-json={\"session\": \"${SESSION}\"}" \
    -F "file=@${FILE}")

STATUS=$(echo "$UPLOAD_RESP" | jq -r '.status')
if [ "$STATUS" != "success" ]; then
    echo "Upload failed:"
    echo "$UPLOAD_RESP" | jq .
    exit 1
fi

SUBID=$(echo "$UPLOAD_RESP" | jq -r '.subid')
echo "Submission ID: ${SUBID}"

# ── Poll submission until a job appears ──────────────────────────────
echo ""
echo "--- Waiting for job ---"
JOB_ID=""
while [ -z "$JOB_ID" ]; do
    if [ "$SECONDS" -ge "$TIMEOUT" ]; then
        echo "Timed out after ${TIMEOUT}s waiting for a job to be created."
        exit 1
    fi

    SUB_RESP=$(curl -s "${BASE_URL}/api/submissions/${SUBID}")
    JOB_ID=$(echo "$SUB_RESP" | jq -r '[.jobs[]? // empty | select(. != null)] | first // empty')

    if [ -z "$JOB_ID" ]; then
        printf "  %3ds  submission queued...\r" "$SECONDS"
        sleep "$POLL_INTERVAL"
    fi
done
echo "  Job created: ${JOB_ID}              "

# ── Poll job until it finishes ───────────────────────────────────────
echo ""
echo "--- Solving ---"
while true; do
    if [ "$SECONDS" -ge "$TIMEOUT" ]; then
        echo "Timed out after ${TIMEOUT}s waiting for solve."
        exit 1
    fi

    JOB_RESP=$(curl -s "${BASE_URL}/api/jobs/${JOB_ID}")
    JOB_STATUS=$(echo "$JOB_RESP" | jq -r '.status')

    if [ "$JOB_STATUS" = "success" ]; then
        echo "  Solved in ${SECONDS}s."
        break
    elif [ "$JOB_STATUS" = "failure" ]; then
        echo "  Solver failed after ${SECONDS}s."
        exit 1
    fi

    printf "  %3ds  status: %s\r" "$SECONDS" "$JOB_STATUS"
    sleep "$POLL_INTERVAL"
done

# ── Print results ────────────────────────────────────────────────────
echo ""
echo "--- Calibration ---"
CAL_RESP=$(curl -s "${BASE_URL}/api/jobs/${JOB_ID}/calibration")
echo "$CAL_RESP" | jq .

echo "--- Objects in field ---"
OBJ_RESP=$(curl -s "${BASE_URL}/api/jobs/${JOB_ID}/objects_in_field")
echo "$OBJ_RESP" | jq .

echo "--- Tags ---"
TAGS_RESP=$(curl -s "${BASE_URL}/api/jobs/${JOB_ID}/machine_tags")
echo "$TAGS_RESP" | jq .
