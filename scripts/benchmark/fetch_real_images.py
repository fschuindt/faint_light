#!/usr/bin/env python3
"""Build the real-camera-frame benchmark set (stdlib only).

Two classes of genuine, linear, unstretched CCD data:
  ztf   64 ZTF r-band science quadrants (Palomar 48", 3072x3080,
        1.01 "/px, ~0.86 deg) from IRSA IBE, random pointings
  tess  64 TESS calibrated FFIs (2048x2048 science area after crop,
        ~21 "/px, ~12 deg) from MAST, 4 sectors x 4 cameras x 4 CCDs

Each frame is rewritten as a minimal single-HDU float32 FITS with ALL
original header cards dropped (no WCS, no RA/DEC hints — blind solving
for every solver). The original WCS center goes into manifest.json as
ground truth, along with the per-image FOV (degrees, image height).

Usage: fetch_real_images.py [--limit-ztf N] [--limit-tess N] [--out DIR]
"""
import argparse
import json
import math
import os
import random
import re
import struct
import sys
import time
import urllib.parse
import urllib.request

UA = {"User-Agent": "faint_light-benchmark/1.0"}
CARD = 80
BLOCK = 2880


# ---------------------------------------------------------------- fits io
def read_blocks(path):
    with open(path, "rb") as f:
        return f.read()


def parse_header(buf, off):
    """Parse header cards starting at off; returns (dict, data_offset)."""
    hdr = {}
    pos = off
    while pos < len(buf):
        card = buf[pos:pos + CARD]
        key = card[:8].decode("ascii", "replace").strip()
        if key == "END":
            pos += CARD
            # data starts at next 2880 boundary
            data_off = ((pos - off + BLOCK - 1) // BLOCK) * BLOCK + off
            return hdr, data_off
        text = card.decode("ascii", "replace")
        if "=" in text[8:10]:
            val = text[10:].split("/")[0].strip()
            hdr[key] = val
        pos += CARD
    raise ValueError("no END card")


def hval_f(hdr, key, default=None):
    v = hdr.get(key)
    if v is None:
        return default
    try:
        return float(v.strip("'\" "))
    except ValueError:
        return default


def hval_i(hdr, key, default=None):
    v = hval_f(hdr, key, None)
    return default if v is None else int(v)


def find_image_hdu(buf):
    """Return (hdr, data_offset) of the first HDU with a 2-D image."""
    off = 0
    while off < len(buf):
        hdr, data_off = parse_header(buf, off)
        naxis = hval_i(hdr, "NAXIS", 0)
        bitpix = hval_i(hdr, "BITPIX", 0)
        if naxis == 2 and hval_i(hdr, "NAXIS1", 0) > 500:
            return hdr, data_off
        # skip data of this HDU
        nelem = 1
        for ax in range(1, naxis + 1):
            nelem *= hval_i(hdr, "NAXIS%d" % ax, 1)
        pcount = hval_i(hdr, "PCOUNT", 0)
        dlen = abs(bitpix) // 8 * (nelem + pcount) if naxis > 0 else 0
        off = data_off + ((dlen + BLOCK - 1) // BLOCK) * BLOCK
    raise ValueError("no 2-D image HDU found")


def card(key, value, comment=""):
    if isinstance(value, bool):
        v = "T" if value else "F"
        s = "%-8s= %20s" % (key, v)
    elif isinstance(value, int):
        s = "%-8s= %20d" % (key, value)
    elif isinstance(value, float):
        s = "%-8s= %20.10G" % (key, value)
    else:
        s = "%-8s= '%s'" % (key, value)
    if comment:
        s += " / " + comment
    return s[:CARD].ljust(CARD).encode("ascii")


def write_min_fits(path, w, h, payload_be_f32):
    """Minimal single-HDU float32 FITS: no WCS, no hints, just pixels."""
    cards = [
        card("SIMPLE", True), card("BITPIX", -32), card("NAXIS", 2),
        card("NAXIS1", w), card("NAXIS2", h),
        card("COMMENT", "faint_light real-frame benchmark; WCS stripped"),
        b"END".ljust(CARD),
    ]
    hdr = b"".join(cards)
    hdr += b" " * ((BLOCK - len(hdr) % BLOCK) % BLOCK)
    data = payload_be_f32
    pad = (BLOCK - len(data) % BLOCK) % BLOCK
    with open(path, "wb") as f:
        f.write(hdr)
        f.write(data)
        f.write(b"\x00" * pad)


def to_f32_be(buf, bitpix, bzero, bscale, n):
    """Convert raw big-endian data of any BITPIX to big-endian float32."""
    if bitpix == -32 and bzero == 0.0 and bscale == 1.0:
        return buf[:4 * n]
    fmt = {8: "%dB", 16: ">%dh", 32: ">%di", -32: ">%df", -64: ">%dd"}[bitpix]
    vals = struct.unpack(fmt % n, buf[:abs(bitpix) // 8 * n])
    if bzero or bscale != 1.0:
        vals = [bzero + bscale * v for v in vals]
    return struct.pack(">%df" % n, *vals)


def fetch(url, path, tries=4):
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=300) as r, open(path, "wb") as f:
                while True:
                    chunk = r.read(1 << 20)
                    if not chunk:
                        break
                    f.write(chunk)
            if os.path.getsize(path) > 100000:
                return True
        except Exception as e:
            print("  retry %d %s: %s" % (attempt + 1, os.path.basename(path), e), flush=True)
            time.sleep(5 * (attempt + 1))
    return False


# ---------------------------------------------------------------- ztf
def ztf_rows(ra, dec):
    url = ("https://irsa.ipac.caltech.edu/ibe/search/ztf/products/sci?POS=%.4f,%.4f&ct=csv"
           % (ra, dec))
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        text = r.read().decode()
    lines = [l for l in text.splitlines() if l.strip()]
    if len(lines) < 2:
        return []
    cols = [c.strip() for c in lines[0].split(",")]
    out = []
    for line in lines[1:]:
        vals = [v.strip() for v in line.split(",")]
        if len(vals) == len(cols):
            out.append(dict(zip(cols, vals)))
    return out


def ztf_url(row):
    ffd = row["filefracday"]
    return ("https://irsa.ipac.caltech.edu/ibe/data/ztf/products/sci/"
            "%s/%s/%s/ztf_%s_%06d_%s_c%02d_o_q%d_sciimg.fits" % (
                ffd[:4], ffd[4:8], ffd[8:], ffd,
                int(row["field"]), row["filtercode"],
                int(row["ccdid"]), int(row["qid"])))


def build_ztf(n, outdir, tmpdir, manifest, rng):
    got = 0
    seen = set()
    while got < n:
        ra = rng.uniform(0, 360)
        dec = math.degrees(math.asin(rng.uniform(math.sin(math.radians(-25)),
                                                 math.sin(math.radians(75)))))
        try:
            rows = ztf_rows(ra, dec)
        except Exception as e:
            print("  ztf query failed (%s), retrying" % e, flush=True)
            time.sleep(5)
            continue
        rows = [r for r in rows if r.get("filtercode") == "zr"
                and r.get("imgtypecode", "o") == "o"]
        if not rows:
            continue
        row = rows[0]
        key = (row["field"], row["ccdid"], row["qid"])
        if key in seen:
            continue
        seen.add(key)
        name = "ztf_%03d" % got
        tmp = os.path.join(tmpdir, name + "_raw.fits")
        if not fetch(ztf_url(row), tmp):
            continue
        try:
            buf = read_blocks(tmp)
            hdr, doff = find_image_hdu(buf)
            w, h = hval_i(hdr, "NAXIS1"), hval_i(hdr, "NAXIS2")
            crval1, crval2 = hval_f(hdr, "CRVAL1"), hval_f(hdr, "CRVAL2")
            # pixel scale from CD matrix
            cd11 = hval_f(hdr, "CD1_1", 0.0) or 0.0
            cd21 = hval_f(hdr, "CD2_1", 0.0) or 0.0
            ps = math.hypot(cd11, cd21) * 3600.0 or 1.01
            fov = h * ps / 3600.0
            bitpix = hval_i(hdr, "BITPIX")
            bzero = hval_f(hdr, "BZERO", 0.0)
            bscale = hval_f(hdr, "BSCALE", 1.0)
            payload = to_f32_be(buf[doff:], bitpix, bzero, bscale, w * h)
            write_min_fits(os.path.join(outdir, name + ".fits"), w, h, payload)
        except Exception as e:
            print("  ztf convert failed %s: %s" % (name, e), flush=True)
            os.remove(tmp)
            continue
        os.remove(tmp)
        manifest.append({"id": name, "file": name + ".fits", "role": "random",
                         "cls": "ztf", "fov": round(fov, 4),
                         "ra": round(crval1, 5), "dec": round(crval2, 5),
                         "pixscale": round(ps, 4), "src": ztf_url(row)})
        got += 1
        print("  ztf %d/%d (%s ra=%.2f dec=%.2f)" % (got, n, name, crval1, crval2), flush=True)


# ---------------------------------------------------------------- tess
def tess_sector_urls(sector):
    url = ("https://archive.stsci.edu/missions/tess/download_scripts/"
           "sector/tesscurl_sector_%d_ffic.sh" % sector)
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=60) as r:
        text = r.read().decode()
    return re.findall(r"https://\S+ffic\.fits", text)


def build_tess(n, outdir, tmpdir, manifest, sectors):
    got = 0
    per = max(1, n // (len(sectors) * 16))
    for sector in sectors:
        if got >= n:
            break
        try:
            urls = tess_sector_urls(sector)
        except Exception as e:
            print("  sector %d list failed: %s" % (sector, e), flush=True)
            continue
        byccd = {}
        for u in urls:
            m = re.search(r"-s\d{4}-(\d)-(\d)-", u)
            if m:
                byccd.setdefault((m.group(1), m.group(2)), []).append(u)
        for (cam, ccd), lst in sorted(byccd.items()):
            if got >= n:
                break
            for k in range(per):
                if got >= n:
                    break
                # middle-of-sector frames avoid start/end artifacts
                u = lst[len(lst) // 2 + k * 7]
                name = "tess_%03d" % got
                tmp = os.path.join(tmpdir, name + "_raw.fits")
                if not fetch(u, tmp):
                    continue
                try:
                    buf = read_blocks(tmp)
                    hdr, doff = find_image_hdu(buf)
                    w, h = hval_i(hdr, "NAXIS1"), hval_i(hdr, "NAXIS2")
                    crval1, crval2 = hval_f(hdr, "CRVAL1"), hval_f(hdr, "CRVAL2")
                    cd11 = hval_f(hdr, "CD1_1", 0.0) or 0.0
                    cd21 = hval_f(hdr, "CD2_1", 0.0) or 0.0
                    ps = math.hypot(cd11, cd21) * 3600.0 or 21.0
                    bitpix = hval_i(hdr, "BITPIX")
                    bzero = hval_f(hdr, "BZERO", 0.0)
                    bscale = hval_f(hdr, "BSCALE", 1.0)
                    payload = to_f32_be(buf[doff:], bitpix, bzero, bscale, w * h)
                    # crop to the 2048x2048 science area (cols 44..2092,
                    # rows 0..2048); frames are 2136x2078 with collateral
                    cw = ch = 2048
                    x0 = 44 if w >= 2092 else 0
                    ch = min(ch, h)
                    rowbytes = w * 4
                    out = bytearray()
                    for r_ in range(ch):
                        s0 = r_ * rowbytes + x0 * 4
                        out += payload[s0:s0 + cw * 4]
                    write_min_fits(os.path.join(outdir, name + ".fits"),
                                   cw, ch, bytes(out))
                    fov = ch * ps / 3600.0
                except Exception as e:
                    print("  tess convert failed %s: %s" % (name, e), flush=True)
                    os.remove(tmp)
                    continue
                os.remove(tmp)
                manifest.append({"id": name, "file": name + ".fits",
                                 "role": "random", "cls": "tess",
                                 "fov": round(fov, 3),
                                 "ra": round(crval1, 5), "dec": round(crval2, 5),
                                 "pixscale": round(ps, 3), "src": u})
                got += 1
                print("  tess %d/%d (%s s%d cam%s ccd%s ra=%.2f dec=%.2f)"
                      % (got, n, name, sector, cam, ccd, crval1, crval2), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.expanduser("~/bench/real_images"))
    ap.add_argument("--limit-ztf", type=int, default=64)
    ap.add_argument("--limit-tess", type=int, default=64)
    ap.add_argument("--sectors", default="60,65,70,75")
    args = ap.parse_args()

    outdir = args.out
    tmpdir = os.path.join(outdir, "tmp")
    os.makedirs(tmpdir, exist_ok=True)
    manifest = []
    mpath = os.path.join(outdir, "manifest.json")
    if os.path.exists(mpath):
        manifest = json.load(open(mpath))

    rng = random.Random(20260728)
    have_ztf = sum(1 for m in manifest if m["cls"] == "ztf")
    have_tess = sum(1 for m in manifest if m["cls"] == "tess")
    print("have %d ztf, %d tess" % (have_ztf, have_tess), flush=True)
    if have_ztf < args.limit_ztf:
        # rebuild ztf from scratch for deterministic ids
        manifest = [m for m in manifest if m["cls"] != "ztf"]
        build_ztf(args.limit_ztf, outdir, tmpdir, manifest, rng)
    if have_tess < args.limit_tess:
        manifest = [m for m in manifest if m["cls"] != "tess"]
        sectors = [int(s) for s in args.sectors.split(",")]
        build_tess(args.limit_tess, outdir, tmpdir, manifest, sectors)

    manifest.sort(key=lambda m: m["id"])
    with open(mpath, "w") as f:
        json.dump(manifest, f, indent=1)
    print("manifest: %d entries -> %s" % (len(manifest), mpath), flush=True)


if __name__ == "__main__":
    main()
