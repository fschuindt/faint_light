#!/usr/bin/env bash
# Put a CMake new enough for FLTK 1.5 (>= 3.28) on PATH.
#
# Ubuntu 22.04 ships 3.22 and 24.04 ships 3.28.3, so rather than depend on
# which image the runner happens to be, fetch a known-good build unless the
# system one is already sufficient.
set -euo pipefail

WANT_MAJOR=3
WANT_MINOR=28
VERSION=3.31.6

have=$(cmake --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+' | head -1 || true)
if [ -n "$have" ]; then
    major=${have%%.*}
    minor=${have##*.}
    if [ "$major" -gt "$WANT_MAJOR" ] ||
       { [ "$major" -eq "$WANT_MAJOR" ] && [ "$minor" -ge "$WANT_MINOR" ]; }; then
        echo "cmake $have is new enough"
        exit 0
    fi
fi

case "$(uname -m)" in
    x86_64)  arch=x86_64 ;;
    aarch64) arch=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

tarball="cmake-${VERSION}-linux-${arch}.tar.gz"
url="https://github.com/Kitware/CMake/releases/download/v${VERSION}/${tarball}"
echo "installing cmake ${VERSION} for ${arch} (found '${have:-none}')"
curl -fsSL "$url" -o "/tmp/${tarball}"
sudo tar -xzf "/tmp/${tarball}" -C /opt
echo "/opt/cmake-${VERSION}-linux-${arch}/bin" >> "$GITHUB_PATH"
