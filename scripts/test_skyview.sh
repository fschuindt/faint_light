#!/bin/bash
#
# Test the skyview endpoint: solve an image, print the JSON result, and
# fetch (and open) the all-sky chart.
#
# Usage: ./test_skyview.sh <image_file> [options]
#
#   -s host:port   server                (default: localhost:8000, or $SKYVIEW_SERVER)
#   -t timestamp   exposure time, UTC    (default: file mtime; Unix secs or YYYY-MM-DDTHH:MM:SSZ)
#   -a lat         site latitude, deg    (default: $SKYVIEW_LAT)
#   -o lon         site longitude, deg   (default: $SKYVIEW_LON, east-positive)
#   -O out.svg     where to save the chart (default: alongside the image)
#   -n             don't open the chart in a viewer
#
# Example:
#   SKYVIEW_LAT=40.7128 SKYVIEW_LON=-74.0060 ./scripts/test_skyview.sh shot.jpg

set -euo pipefail

usage() { grep '^#' "$0" | sed 's/^# \{0,1\}//' | tail -n +2; exit 1; }

FILE="${1:-}"
[ -z "$FILE" ] && usage
[ "$FILE" = "-h" ] || [ "$FILE" = "--help" ] && usage
shift

SERVER="${SKYVIEW_SERVER:-localhost:8000}"
LAT="${SKYVIEW_LAT:-}"
LON="${SKYVIEW_LON:-}"
TIMESTAMP=""
OUT=""
OPEN=1

while getopts "s:t:a:o:O:nh" opt; do
    case "$opt" in
        s) SERVER="$OPTARG" ;;
        t) TIMESTAMP="$OPTARG" ;;
        a) LAT="$OPTARG" ;;
        o) LON="$OPTARG" ;;
        O) OUT="$OPTARG" ;;
        n) OPEN=0 ;;
        *) usage ;;
    esac
done

if [ ! -f "$FILE" ]; then
    echo "Error: file '${FILE}' not found"
    exit 1
fi

if ! command -v jq &>/dev/null; then
    echo "Error: jq is required (sudo pacman -S jq)"
    exit 1
fi

if [ -z "$LAT" ] || [ -z "$LON" ]; then
    echo "Error: observing site not set."
    echo "Pass -a <lat> -o <lon>, or export SKYVIEW_LAT / SKYVIEW_LON."
    exit 1
fi

# Default the timestamp to the file's modification time — usually close to
# the exposure time for a freshly copied image. Override with -t.
if [ -z "$TIMESTAMP" ]; then
    TIMESTAMP=$(date -u -r "$FILE" +%Y-%m-%dT%H:%M:%SZ)
    echo "Note: no -t given, using file mtime ${TIMESTAMP} as exposure time."
fi

[ -z "$OUT" ] && OUT="${FILE%.*}.chart.svg"
BASE_URL="http://${SERVER}"

echo "--- Skyview: $(basename "$FILE") @ ${TIMESTAMP}, lat ${LAT}°, lon ${LON}° → ${SERVER} ---"
RESP=$(curl -s --fail-with-body -X POST "${BASE_URL}/api/skyview" \
    -F "file=@${FILE}" \
    -F "timestamp=${TIMESTAMP}" \
    -F "latitude=${LAT}" \
    -F "longitude=${LON}") || {
    if [ -z "$RESP" ]; then
        echo "Error: no response from ${BASE_URL} — is the server running?"
        echo "Start it with: docker compose up -d   (or: FAINT_LIGHT_INDEX_DIR=./indexes cargo run --release -p fl-server)"
    else
        echo "$RESP" | jq . 2>/dev/null || echo "$RESP"
    fi
    exit 1
}

echo "$RESP" | jq .

if [ "$(echo "$RESP" | jq -r '.field.above_horizon')" = "false" ]; then
    echo ""
    echo "Warning: solved field is below the horizon — check timestamp and coordinates."
fi

CHART_URL=$(echo "$RESP" | jq -r '.sky_chart_url')
curl -s "${BASE_URL}${CHART_URL}" -o "$OUT"
echo ""
echo "Sky chart: ${BASE_URL}${CHART_URL}"
echo "Saved to:  ${OUT}"

if [ "$OPEN" = 1 ] && command -v xdg-open &>/dev/null && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    xdg-open "$OUT" >/dev/null 2>&1 &
fi
