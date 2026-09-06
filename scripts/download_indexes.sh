#!/bin/bash
#
# Download astrometry.net index files for:
#   Canon T3i (22.3 x 14.9 mm sensor) + 400mm → FOV ~3.2deg (192 arcmin)
#   Canon T3i (22.3 x 14.9 mm sensor) + 40mm  → FOV ~31deg (1868 arcmin)
#
# Index scale coverage (4100 series, Tycho-2 catalog):
#   4107-4109: 22-60' (small FOV ~1deg)
#   4110:   60-85'   (1.0-1.4deg)  |
#   4111:   85-120'  (1.4-2.0deg)  |
#   4112:  120-170'  (2.0-2.8deg)  | 400mm (FOV ~3.2deg)
#   4113:  170-240'  (2.8-4.0deg)  |
#   4114:  240-340'  (4.0-5.6deg)  |
#   4115:  340-480'  (5.6-8.0deg)
#   4116:  480-680'  (8.0-11.3deg)
#   4117:  680-1000' (11.3-16.7deg) |
#   4118: 1000-1400' (16.7-23.3deg) | 40mm (FOV ~31deg)
#   4119: 1400-2000' (23.3-33.3deg) |

set -e

BASE_URL="https://data.astrometry.net/4100"
DEST="./indexes"

mkdir -p "$DEST"

for i in $(seq 4107 4119); do
    echo "Downloading index-${i}.fits ..."
    wget -c -P "$DEST" "${BASE_URL}/index-${i}.fits"
done

# Deeper 4200-series (Tycho-2 + 2MASS) small-quad bands, added 2026-07-29
# for sparse sub-degree fields (quads 8-22'). ~2.1 GB total.
BASE_4200="https://data.astrometry.net/4200"
for i in $(seq -w 0 47); do
    wget -c -P "$DEST" "${BASE_4200}/index-4204-${i}.fits"
done
for i in $(seq -w 0 11); do
    wget -c -P "$DEST" "${BASE_4200}/index-4205-${i}.fits"
    wget -c -P "$DEST" "${BASE_4200}/index-4206-${i}.fits"
done

# Modern Gaia-based 6000/6100 series (compact, all scale bands),
# added 2026-07-29. ~2.6 GB total.
for series in 6000 6100; do
    BASE="https://data.astrometry.net/${series}"
    LIST=$(wget -qO- "$BASE/" | grep -oE "href=\"index-[^\"]*\.fits\"" | sed "s/href=\"//;s/\"//" | sort -u)
    for f in $LIST; do
        wget -c -P "$DEST" "${BASE}/${f}"
    done
done

echo "Done. Index files saved to ${DEST}/"
