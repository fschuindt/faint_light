#!/usr/bin/env bash
# Head-to-head: submit the same image(s) to two nova-API servers and compare
# wall time and calibration.
#
# usage: ./scripts/bench_vs_astrometry.sh <faint_light_host:port> <astrometry_host:port> <image> [image...]
set -euo pipefail

if [ $# -lt 3 ]; then
    echo "usage: $0 <faint_light_host:port> <astrometry_host:port> <image> [image...]" >&2
    exit 2
fi

HOST_A="$1"
HOST_B="$2"
shift 2

solve_one() {
    local host="$1" img="$2"
    local t0 t1 session subid jobid status
    t0=$(date +%s.%N)
    # faint_light ignores the apikey; the stock test container validates it
    # against its fixture user, hence the fixed key.
    session=$(curl -sL -X POST "http://$host/api/login" \
        --data-urlencode "request-json={\"apikey\":\"${APIKEY:-1jcrmadfnxngxscd}\"}" | jq -r .session)
    subid=$(curl -sL -X POST "http://$host/api/upload" \
        -F "request-json={\"session\":\"$session\"};type=text/plain" \
        -F "file=@$img" | jq -r .subid)
    jobid=null
    while [ "$jobid" = "null" ] || [ -z "$jobid" ]; do
        sleep 0.2
        jobid=$(curl -sL "http://$host/api/submissions/$subid" | jq -r '.jobs[0]')
    done
    status=solving
    while [ "$status" = "solving" ]; do
        sleep 0.2
        status=$(curl -sL "http://$host/api/jobs/$jobid" | jq -r .status)
    done
    t1=$(date +%s.%N)
    local cal
    cal=$(curl -sL "http://$host/api/jobs/$jobid/calibration" |
        jq -r '"ra=\(.ra|.*1000|round/1000) dec=\(.dec|.*1000|round/1000) scale=\(.pixscale|.*1000|round/1000) orient=\(.orientation|.*100|round/100) parity=\(.parity)"' 2>/dev/null || echo "n/a")
    printf "%-8s %7.2fs  %s\n" "$status" "$(awk "BEGIN{printf \"%.2f\", $t1 - $t0}")" "$cal"
}

for img in "$@"; do
    name=$(basename "$img")
    echo "== $name"
    printf "  faint_light : "
    solve_one "$HOST_A" "$img"
    printf "  astrometry  : "
    solve_one "$HOST_B" "$img"
done
