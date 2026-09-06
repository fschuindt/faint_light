#!/usr/bin/env python3
"""Build the 256-image benchmark dataset (runs inside the twirl_solver image,
which already carries numpy + astropy).

  small  (0.86 deg, 1.01"/px)  86 x ZTF r-band science quadrants   3072x3080
  medium (5.85 deg, 20.6"/px)  85 x TESS FFI central crop          1024x1024
  large  (11.7 deg, 20.6"/px)  85 x TESS FFI science area          2048x2048

Every image is written as a MINIMAL 16-bit FITS: all original header cards
are dropped, so there is no WCS, no RA/DEC, no scale - solving is blind by
construction. The pipeline WCS of the source frame is recorded in the
manifest as ground truth. 16-bit is the common denominator every candidate
accepts (PlateSolve3/ASPS are the fussiest).

Existing WCS-stripped float32 frames from the earlier run are reused where
possible (they already carry their truth in a manifest); the rest are pulled
fresh from IRSA (ZTF) and MAST (TESS).
"""
import json
import math
import os
import re
import struct
import sys
import time
import urllib.request

import numpy as np
from astropy.io import fits
from astropy.wcs import WCS

OUT = "/data/images"
OLD = "/old"           # previous real_images dir (float32, WCS stripped)
TMP = "/data/tmp"
UA = {"User-Agent": "faint_light-benchmark/2.0"}
N_SMALL, N_MED, N_LARGE = 86, 85, 85
CARD, BLOCK = 80, 2880


def card(k, v):
    if isinstance(v, bool):
        s = "%-8s= %20s" % (k, "T" if v else "F")
    elif isinstance(v, int):
        s = "%-8s= %20d" % (k, v)
    elif isinstance(v, float):
        s = "%-8s= %20.10G" % (k, v)
    else:
        s = "%-8s= '%s'" % (k, v)
    return s.ljust(CARD).encode()


def write_fits16(path, arr):
    """Minimal 16-bit FITS, linearly scaled, no WCS/metadata whatsoever."""
    a = np.asarray(arr, dtype=float)
    finite = np.isfinite(a)
    if not finite.all():
        a = np.where(finite, a, np.nanmedian(a[finite]) if finite.any() else 0.0)
    lo = np.percentile(a, 0.05)
    hi = np.percentile(a, 99.95)
    if hi <= lo:
        lo, hi = a.min(), max(a.max(), a.min() + 1)
    scaled = np.clip((a - lo) / (hi - lo), 0, 1) * 65535.0
    ints = (scaled.astype(np.int32) - 32768).astype(">i2")
    h, w = ints.shape
    cards = [card("SIMPLE", True), card("BITPIX", 16), card("NAXIS", 2),
             card("NAXIS1", int(w)), card("NAXIS2", int(h)),
             card("BZERO", 32768.0), card("BSCALE", 1.0),
             card("COMMENT", "faint_light benchmark: header stripped for blind solving"),
             b"END".ljust(CARD)]
    hdr = b"".join(cards)
    hdr += b" " * ((BLOCK - len(hdr) % BLOCK) % BLOCK)
    data = ints.tobytes()
    with open(path, "wb") as f:
        f.write(hdr)
        f.write(data)
        f.write(b"\x00" * ((BLOCK - len(data) % BLOCK) % BLOCK))


def fetch(url, path, tries=4):
    for a in range(tries):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=600) as r, open(path, "wb") as f:
                while True:
                    c = r.read(1 << 20)
                    if not c:
                        break
                    f.write(c)
            if os.path.getsize(path) > 100000:
                return True
        except Exception as e:
            print("   retry %d %s: %s" % (a + 1, os.path.basename(path), e), flush=True)
            time.sleep(4 * (a + 1))
    return False


# ---------------------------------------------------------------- sources
def ztf_rows(ra, dec):
    url = ("https://irsa.ipac.caltech.edu/ibe/search/ztf/products/sci?POS=%.4f,%.4f&ct=csv"
           % (ra, dec))
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=90) as r:
        text = r.read().decode()
    lines = [l for l in text.splitlines() if l.strip()]
    if len(lines) < 2:
        return []
    cols = [c.strip() for c in lines[0].split(",")]
    return [dict(zip(cols, [v.strip() for v in l.split(",")]))
            for l in lines[1:] if len(l.split(",")) == len(cols)]


def ztf_url(row):
    f = row["filefracday"]
    return ("https://irsa.ipac.caltech.edu/ibe/data/ztf/products/sci/"
            "%s/%s/%s/ztf_%s_%06d_%s_c%02d_o_q%d_sciimg.fits"
            % (f[:4], f[4:8], f[8:], f, int(row["field"]), row["filtercode"],
               int(row["ccdid"]), int(row["qid"])))


def tess_urls(sector):
    url = ("https://archive.stsci.edu/missions/tess/download_scripts/"
           "sector/tesscurl_sector_%d_ffic.sh" % sector)
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=90) as r:
        return re.findall(r"https://\S+ffic\.fits", r.read().decode())


def main():
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(TMP, exist_ok=True)
    manifest = []
    mpath = "/data/manifest.json"
    if os.path.exists(mpath):
        manifest = json.load(open(mpath))
    have = {m["id"] for m in manifest}

    old_manifest = {}
    if os.path.exists(os.path.join(OLD, "manifest.json")):
        for m in json.load(open(os.path.join(OLD, "manifest.json"))):
            old_manifest[m["id"]] = m

    rng = np.random.RandomState(20260730)

    # ---------------- small: ZTF quadrants ----------------
    n = sum(1 for m in manifest if m["cls"] == "small")
    reused = [m for m in old_manifest.values() if m["cls"] == "ztf"]
    for src in reused:
        if n >= N_SMALL:
            break
        sid = "small_%03d" % n
        if sid in have:
            n += 1
            continue
        p = os.path.join(OLD, src["file"])
        if not os.path.exists(p):
            continue
        data = fits.open(p)[0].data
        write_fits16(os.path.join(OUT, sid + ".fits"), data)
        manifest.append({"id": sid, "file": sid + ".fits", "cls": "small",
                         "fov_deg": round(src["fov"], 4), "pixscale": src["pixscale"],
                         "ra": src["ra"], "dec": src["dec"], "width": 3072, "height": 3080,
                         "source": src.get("src", "ZTF sciimg (reused)"),
                         "instrument": "ZTF / Palomar 48in"})
        n += 1
        print("small %d/%d (reused %s)" % (n, N_SMALL, src["id"]), flush=True)

    seen_fields = set()
    while n < N_SMALL:
        ra = float(rng.uniform(0, 360))
        dec = math.degrees(math.asin(float(rng.uniform(math.sin(math.radians(-25)),
                                                       math.sin(math.radians(75))))))
        try:
            rows = [r for r in ztf_rows(ra, dec)
                    if r.get("filtercode") == "zr" and r.get("imgtypecode", "o") == "o"]
        except Exception as e:
            print("   ztf query: %s" % e, flush=True)
            time.sleep(4)
            continue
        if not rows:
            continue
        row = rows[0]
        key = (row["field"], row["ccdid"], row["qid"])
        if key in seen_fields:
            continue
        seen_fields.add(key)
        raw = os.path.join(TMP, "ztf_raw.fits")
        if not fetch(ztf_url(row), raw):
            continue
        try:
            hdu = fits.open(raw)[0]
            w = WCS(hdu.header)
            ny, nx = hdu.data.shape
            sky = w.pixel_to_world(nx / 2.0, ny / 2.0)
            ps = float(np.sqrt(np.abs(np.linalg.det(w.pixel_scale_matrix))) * 3600.0)
            sid = "small_%03d" % n
            write_fits16(os.path.join(OUT, sid + ".fits"), hdu.data)
            manifest.append({"id": sid, "file": sid + ".fits", "cls": "small",
                             "fov_deg": round(ny * ps / 3600.0, 4), "pixscale": round(ps, 4),
                             "ra": round(float(sky.ra.deg), 5), "dec": round(float(sky.dec.deg), 5),
                             "width": int(nx), "height": int(ny),
                             "source": ztf_url(row), "instrument": "ZTF / Palomar 48in"})
            n += 1
            print("small %d/%d (%s)" % (n, N_SMALL, sid), flush=True)
        except Exception as e:
            print("   ztf convert: %s" % e, flush=True)
        finally:
            if os.path.exists(raw):
                os.remove(raw)

    json.dump(manifest, open(mpath, "w"), indent=1)

    # ---------------- large + medium: TESS FFIs ----------------
    # medium is the central 1024^2 crop of the same frame used for large, so the
    # two classes probe identical sky positions at different field sizes.
    nl = sum(1 for m in manifest if m["cls"] == "large")
    old_tess = [m for m in old_manifest.values() if m["cls"] == "tess"]
    for src in old_tess:
        if nl >= N_LARGE:
            break
        lid, mid = "large_%03d" % nl, "medium_%03d" % nl
        p = os.path.join(OLD, src["file"])
        if not os.path.exists(p) or lid in have:
            nl += 1
            continue
        data = np.asarray(fits.open(p)[0].data, dtype=float)
        write_fits16(os.path.join(OUT, lid + ".fits"), data)
        h, w_ = data.shape
        c = data[h // 2 - 512:h // 2 + 512, w_ // 2 - 512:w_ // 2 + 512]
        write_fits16(os.path.join(OUT, mid + ".fits"), c)
        ps = src["pixscale"]
        manifest.append({"id": lid, "file": lid + ".fits", "cls": "large",
                         "fov_deg": round(h * ps / 3600.0, 4), "pixscale": ps,
                         "ra": src["ra"], "dec": src["dec"], "width": int(w_), "height": int(h),
                         "source": src.get("src", "TESS FFI (reused)"), "instrument": "TESS"})
        manifest.append({"id": mid, "file": mid + ".fits", "cls": "medium",
                         "fov_deg": round(1024 * ps / 3600.0, 4), "pixscale": ps,
                         "ra": src["ra"], "dec": src["dec"], "width": 1024, "height": 1024,
                         "source": src.get("src", "TESS FFI (reused)") + " [central 1024 crop]",
                         "instrument": "TESS"})
        nl += 1
        print("large+medium %d/%d (reused %s)" % (nl, N_LARGE, src["id"]), flush=True)

    json.dump(manifest, open(mpath, "w"), indent=1)

    sectors = [62, 67, 72, 77]
    si = 0
    while nl < N_LARGE and si < len(sectors) * 40:
        sector = sectors[si % len(sectors)]
        try:
            urls = tess_urls(sector)
        except Exception as e:
            print("   sector %d: %s" % (sector, e), flush=True)
            si += 1
            continue
        pick = urls[len(urls) // 3 + (si // len(sectors)) * 11]
        si += 1
        raw = os.path.join(TMP, "tess_raw.fits")
        if not fetch(pick, raw):
            continue
        try:
            hdul = fits.open(raw)
            hdu = None
            for h_ in hdul:
                if getattr(h_, "data", None) is not None and h_.data.ndim == 2 and h_.data.shape[0] > 500:
                    hdu = h_
                    break
            w = WCS(hdu.header)
            data = np.asarray(hdu.data, dtype=float)
            ny, nx = data.shape
            x0 = 44 if nx >= 2092 else 0
            sci = data[0:min(2048, ny), x0:x0 + 2048]
            sh, sw = sci.shape
            sky = w.pixel_to_world(x0 + sw / 2.0, sh / 2.0)
            ps = float(np.sqrt(np.abs(np.linalg.det(w.pixel_scale_matrix))) * 3600.0)
            lid, mid = "large_%03d" % nl, "medium_%03d" % nl
            write_fits16(os.path.join(OUT, lid + ".fits"), sci)
            c = sci[sh // 2 - 512:sh // 2 + 512, sw // 2 - 512:sw // 2 + 512]
            write_fits16(os.path.join(OUT, mid + ".fits"), c)
            for cls, fid, size in (("large", lid, sh), ("medium", mid, 1024)):
                manifest.append({"id": fid, "file": fid + ".fits", "cls": cls,
                                 "fov_deg": round(size * ps / 3600.0, 4), "pixscale": round(ps, 4),
                                 "ra": round(float(sky.ra.deg), 5), "dec": round(float(sky.dec.deg), 5),
                                 "width": int(size), "height": int(size),
                                 "source": pick + ("" if cls == "large" else " [central 1024 crop]"),
                                 "instrument": "TESS"})
            nl += 1
            print("large+medium %d/%d (%s)" % (nl, N_LARGE, lid), flush=True)
        except Exception as e:
            print("   tess convert: %s" % e, flush=True)
        finally:
            if os.path.exists(raw):
                os.remove(raw)
        json.dump(manifest, open(mpath, "w"), indent=1)

    manifest.sort(key=lambda m: m["id"])
    json.dump(manifest, open(mpath, "w"), indent=1)
    with open("/data/manifest.csv", "w") as f:
        cols = ["id", "file", "cls", "fov_deg", "pixscale", "ra", "dec", "width", "height",
                "instrument", "source"]
        f.write(",".join(cols) + "\n")
        for m in manifest:
            f.write(",".join('"%s"' % str(m.get(c, "")) if c in ("source", "instrument")
                             else str(m.get(c, "")) for c in cols) + "\n")
    counts = {}
    for m in manifest:
        counts[m["cls"]] = counts.get(m["cls"], 0) + 1
    print("DATASET DONE:", counts, "total", len(manifest), flush=True)


if __name__ == "__main__":
    main()
