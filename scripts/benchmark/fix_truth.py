#!/usr/bin/env python3
"""Recompute ground truth for the TESS-derived images.

The 64 frames reused from the earlier run carried their *frame* CRVAL as
truth, which sits ~11 arcmin from the centre of the 2048^2 science-area crop
we actually benchmark. Every solver therefore appeared to be ~680" off on
medium/large. This refetches just the FITS header of each source FFI (HTTP
range request, no need for the 35 MB of pixels) and recomputes the true sky
position of the crop centre.

medium_NNN is the central 1024^2 crop of large_NNN, so both share a centre.
"""
import json
import os
import urllib.request

from astropy.io import fits
from astropy.wcs import WCS
import warnings

warnings.filterwarnings("ignore")

MAN = "/data/manifest.json"
TMP = "/data/tmp/hdr.fits"
UA = {"User-Agent": "faint_light-benchmark/2.0", "Range": "bytes=0-2500000"}

manifest = json.load(open(MAN))
by_id = {m["id"]: m for m in manifest}
os.makedirs("/data/tmp", exist_ok=True)

targets = [m for m in manifest if m["cls"] == "large" and m["source"].startswith("http")]
print("recomputing truth for %d TESS pairs" % len(targets), flush=True)

fixed = 0
for m in targets:
    url = m["source"].split(" [")[0]
    try:
        req = urllib.request.Request(url, headers=UA)
        with urllib.request.urlopen(req, timeout=180) as r, open(TMP, "wb") as f:
            f.write(r.read())
        with fits.open(TMP, ignore_missing_end=True, lazy_load_hdus=True) as hdul:
            hdr = None
            for h in hdul:
                if h.header.get("NAXIS") == 2 and h.header.get("NAXIS1", 0) > 500:
                    hdr = h.header
                    break
            if hdr is None:
                print("  no image header: %s" % m["id"], flush=True)
                continue
            w = WCS(hdr)
            nx = hdr["NAXIS1"]
            x0 = 44 if nx >= 2092 else 0
            sky = w.pixel_to_world(x0 + 1024.0, 1024.0)
            ra, dec = round(float(sky.ra.deg), 5), round(float(sky.dec.deg), 5)
    except Exception as e:
        print("  %s: %s" % (m["id"], e), flush=True)
        continue
    old = (m["ra"], m["dec"])
    m["ra"], m["dec"] = ra, dec
    mid = m["id"].replace("large_", "medium_")
    if mid in by_id:
        by_id[mid]["ra"], by_id[mid]["dec"] = ra, dec
    fixed += 1
    if fixed % 10 == 0:
        print("  %d/%d (%s: %.4f,%.4f -> %.4f,%.4f)" % (fixed, len(targets), m["id"],
                                                        old[0], old[1], ra, dec), flush=True)

json.dump(manifest, open(MAN, "w"), indent=1)
cols = ["id", "file", "cls", "fov_deg", "pixscale", "ra", "dec", "width", "height",
        "instrument", "source"]
with open("/data/manifest.csv", "w") as f:
    f.write(",".join(cols) + "\n")
    for m in sorted(manifest, key=lambda x: x["id"]):
        f.write(",".join('"%s"' % str(m.get(c, "")) if c in ("source", "instrument")
                         else str(m.get(c, "")) for c in cols) + "\n")
print("TRUTH FIXED: %d/%d" % (fixed, len(targets)), flush=True)
