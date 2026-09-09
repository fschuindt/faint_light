#!/usr/bin/env bash
# Run a freshly built binary and prove it works and says nothing alarming.
#
# Arguments: the directory the release archive was unpacked into.
# Environment: KIND=headless|gui, OS=Linux|Windows (runner.os).
#
# The fake solver stands in for index files, so this needs nothing but the
# binary and the repository's own test image.
set -euo pipefail

dist="${1:?usage: verify-binary.sh <unpacked-dir>}"
port=7333
base="http://127.0.0.1:${port}"

bin=$(find "$dist" -maxdepth 2 -type f \
    \( -name 'faint-light' -o -name 'faint-light.exe' \
    -o -name 'faint-light-gui' -o -name 'faint-light-gui.exe' \) | head -1)
[ -n "$bin" ] || { echo "no binary found under $dist"; exit 1; }
chmod +x "$bin" 2>/dev/null || true
echo "verifying: $bin"

export FAINT_LIGHT_FAKE=1
export FAINT_LIGHT_PORT="$port"
export FAINT_LIGHT_BIND=127.0.0.1
export RUST_LOG=info

console=$(mktemp)
if [ "$KIND" = gui ] && [ "$OS" = Linux ]; then
    xvfb-run -a "$bin" >"$console" 2>&1 &
else
    "$bin" >"$console" 2>&1 &
fi
pid=$!

# Git Bash job ids are not Windows process ids, so `taskkill //PID` cannot
# be used here; the shell's own kill knows how to reach its child on both
# platforms. Never `wait`: if the signal does not land, the job hangs.
stop() {
    [ -n "${pid:-}" ] || return 0
    kill "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5; do
        kill -0 "$pid" 2>/dev/null || return 0
        sleep 1
    done
    kill -9 "$pid" 2>/dev/null || true
}
trap stop EXIT

# The GUI starts its server on its own, because "start when this window
# opens" is on by default.
ready=0
for _ in $(seq 1 60); do
    if curl -fsS "$base/" >/dev/null 2>&1; then ready=1; break; fi
    sleep 1
done
if [ "$ready" -ne 1 ]; then
    echo "FAIL: nothing answered on $base after 60s"
    echo "--- console ---"; cat "$console"
    exit 1
fi

fail=0
check() { # description, expected substring, curl args...
    local what="$1" want="$2"; shift 2
    local body
    if ! body=$(curl -fsS "$@" 2>&1); then
        echo "FAIL: $what — request failed: $body"; fail=1; return
    fi
    case "$body" in
        *"$want"*) echo "ok: $what" ;;
        *) echo "FAIL: $what — no '$want' in: $body"; fail=1 ;;
    esac
}

check "banner"        "faint_light"        "$base/"
check "nova root"     "astrometry.net"     "$base/nova"
check "nova login"    '"status":"success"' -X POST "$base/nova/api/login" -F 'request-json={}'
check "v1 solve"      '"status":"success"' -X POST "$base/api/v1/solve" \
      -F "file=@testdata/bench/base_m.jpg"
check "v1 solve+fits" '"fits_url"'         -X POST "$base/api/v1/solve" \
      -F "file=@testdata/bench/base_m.jpg" -F fits=1

stop
trap - EXIT
sleep 1

# What the program said for itself. A Windows GUI has no console, so read the
# log file it keeps beside its settings.
logs="$console"
if [ "$KIND" = gui ]; then
    if [ "$OS" = Windows ]; then
        gui_log="$(cygpath -u "$APPDATA")/faint_light/faint-light.log"
    else
        gui_log="${XDG_CONFIG_HOME:-$HOME/.config}/faint_light/faint-light.log"
    fi
    if [ -f "$gui_log" ]; then
        cat "$gui_log" >>"$console"
    elif [ ! -s "$console" ]; then
        echo "FAIL: the GUI produced no log at $gui_log"; fail=1
    fi
fi

echo "--- output ---"
cat "$logs"
echo "--------------"

# The fake solver announces itself; anything else at WARN or above is a
# problem worth failing the release for.
suspect=$(grep -Ei ' (WARN|ERROR) |warning:|panicked' "$logs" \
    | grep -v 'FAINT_LIGHT_FAKE=1' || true)
if [ -n "$suspect" ]; then
    echo "FAIL: unexpected warnings:"
    echo "$suspect"
    fail=1
fi

[ "$fail" -eq 0 ] && echo "PASS: $bin"
exit "$fail"
