//! TAN (gnomonic) WCS: projection, inverse, and least-squares fitting from
//! matched (star xyz, pixel) pairs — a port of astrometry.net's
//! fit_tan_wcs_solve (Procrustes via 2x2 SVD).

use fl_fits::write::Card;

/// ra/dec in degrees to unit vector.
pub fn radec_to_xyz(ra_deg: f64, dec_deg: f64) -> [f64; 3] {
    let (ra, dec) = (ra_deg.to_radians(), dec_deg.to_radians());
    [
        dec.cos() * ra.cos(),
        dec.cos() * ra.sin(),
        dec.sin(),
    ]
}

pub fn xyz_to_radec(v: [f64; 3]) -> (f64, f64) {
    let ra = v[1].atan2(v[0]).to_degrees().rem_euclid(360.0);
    let dec = v[2].clamp(-1.0, 1.0).asin().to_degrees();
    (ra, dec)
}

pub fn normalize(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / n, v[1] / n, v[2] / n]
}

#[inline]
pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
pub fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Angular distance (rad) corresponding to a unit-sphere chord² distance.
pub fn dist2_to_rad(d2: f64) -> f64 {
    (1.0 - d2 / 2.0).clamp(-1.0, 1.0).acos()
}

/// Chord² distance corresponding to an angular distance (rad).
pub fn rad_to_dist2(r: f64) -> f64 {
    let s = (r / 2.0).sin();
    4.0 * s * s
}

/// Tangent-plane projection of unit vector `s` around reference `r`
/// (astrometry.net star_coords, tangent=true). Output (x=east, y=north)
/// in radians. None if `s` is on the far hemisphere.
pub fn tan_project(s: [f64; 3], r: [f64; 3]) -> Option<(f64, f64)> {
    let sdotr = dot(s, r);
    if sdotr <= 0.0 {
        return None;
    }
    if r[2].abs() >= 1.0 {
        // exactly at a pole
        let inv = 1.0 / s[2];
        return Some(if r[2] > 0.0 {
            (s[0] * inv, s[1] * inv)
        } else {
            (-s[0] * inv, s[1] * inv)
        });
    }
    // east vector (unit, perpendicular to r, in z=0 plane)
    let en = (r[0] * r[0] + r[1] * r[1]).sqrt();
    let e = [-r[1] / en, r[0] / en, 0.0];
    // north vector = r x e
    let n = [
        -r[2] * e[1],
        r[2] * e[0],
        r[0] * e[1] - r[1] * e[0],
    ];
    let inv = 1.0 / sdotr;
    Some((dot(s, e) * inv, dot(s, n) * inv))
}

/// Inverse of tan_project: tangent-plane (x=east, y=north) radians → xyz.
pub fn tan_unproject(x: f64, y: f64, r: [f64; 3]) -> [f64; 3] {
    if r[2].abs() >= 1.0 {
        let (sx, sy) = if r[2] > 0.0 { (x, y) } else { (-x, y) };
        return normalize([sx, sy, r[2].signum()]);
    }
    let en = (r[0] * r[0] + r[1] * r[1]).sqrt();
    let e = [-r[1] / en, r[0] / en, 0.0];
    let n = [
        -r[2] * e[1],
        r[2] * e[0],
        r[0] * e[1] - r[1] * e[0],
    ];
    normalize([
        r[0] + x * e[0] + y * n[0],
        r[1] + x * e[1] + y * n[1],
        r[2] + x * e[2] + y * n[2],
    ])
}

#[derive(Debug, Clone, Copy)]
pub struct TanWcs {
    /// Tangent point (ra, dec) degrees.
    pub crval: [f64; 2],
    /// Reference pixel (same 0-based pixel-center convention as the star
    /// extractor).
    pub crpix: [f64; 2],
    /// Degrees per pixel: (east, north) = CD * (pixel - crpix).
    pub cd: [[f64; 2]; 2],
}

impl TanWcs {
    pub fn det_cd(&self) -> f64 {
        self.cd[0][0] * self.cd[1][1] - self.cd[0][1] * self.cd[1][0]
    }

    /// arcsec / pixel
    pub fn pixscale(&self) -> f64 {
        3600.0 * self.det_cd().abs().sqrt()
    }

    /// +1 / -1 from the sign of det(CD) (nova convention).
    pub fn parity(&self) -> f64 {
        if self.det_cd() >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }

    /// Position angle of the image "up" direction, degrees E of N
    /// (astrometry.net tan_get_orientation).
    pub fn orientation(&self) -> f64 {
        let parity = self.parity();
        let t = parity * self.cd[0][0] + self.cd[1][1];
        let a = parity * self.cd[1][0] - self.cd[0][1];
        -a.atan2(t).to_degrees()
    }

    pub fn pixel_to_xyz(&self, px: f64, py: f64) -> [f64; 3] {
        let u = px - self.crpix[0];
        let v = py - self.crpix[1];
        let x = (self.cd[0][0] * u + self.cd[0][1] * v).to_radians();
        let y = (self.cd[1][0] * u + self.cd[1][1] * v).to_radians();
        tan_unproject(x, y, radec_to_xyz(self.crval[0], self.crval[1]))
    }

    pub fn pixel_to_radec(&self, px: f64, py: f64) -> (f64, f64) {
        xyz_to_radec(self.pixel_to_xyz(px, py))
    }

    pub fn xyz_to_pixel(&self, s: [f64; 3]) -> Option<(f64, f64)> {
        let r = radec_to_xyz(self.crval[0], self.crval[1]);
        let (x, y) = tan_project(s, r)?;
        let (xd, yd) = (x.to_degrees(), y.to_degrees());
        let det = self.det_cd();
        if det == 0.0 {
            return None;
        }
        let inv = 1.0 / det;
        let u = inv * (self.cd[1][1] * xd - self.cd[0][1] * yd);
        let v = inv * (-self.cd[1][0] * xd + self.cd[0][0] * yd);
        Some((self.crpix[0] + u, self.crpix[1] + v))
    }

    /// The solution as FITS header cards, in the order and precision
    /// astrometry.net writes into a `.wcs` file. `width`/`height` are the
    /// image dimensions in pixels, recorded as IMAGEW/IMAGEH the way
    /// solve-field does.
    pub fn fits_cards(&self, width: f64, height: f64) -> Vec<Card> {
        vec![
            Card::int("WCSAXES", 2, "number of celestial axes"),
            Card::decimal("EQUINOX", 2000.0, 1, "equinox of the reference frame"),
            Card::string("RADESYS", "FK5", "reference frame"),
            Card::decimal("LONPOLE", 180.0, 1, ""),
            Card::decimal("LATPOLE", 0.0, 1, ""),
            Card::string("CTYPE1", "RA---TAN", "TAN (gnomonic) projection"),
            Card::string("CTYPE2", "DEC--TAN", "TAN (gnomonic) projection"),
            Card::string("CUNIT1", "deg", "axis unit"),
            Card::string("CUNIT2", "deg", "axis unit"),
            Card::float("CRVAL1", self.crval[0], "RA of the reference point"),
            Card::float("CRVAL2", self.crval[1], "DEC of the reference point"),
            // FITS pixel indices start at 1; ours start at 0.
            Card::float("CRPIX1", self.crpix[0] + 1.0, "X reference pixel"),
            Card::float("CRPIX2", self.crpix[1] + 1.0, "Y reference pixel"),
            Card::float("CD1_1", self.cd[0][0], "transformation matrix"),
            Card::float("CD1_2", self.cd[0][1], ""),
            Card::float("CD2_1", self.cd[1][0], ""),
            Card::float("CD2_2", self.cd[1][1], ""),
            Card::decimal("IMAGEW", width, 1, "image width, pixels"),
            Card::decimal("IMAGEH", height, 1, "image height, pixels"),
        ]
    }
}

/// True for header keywords that describe a celestial WCS.
///
/// Writing a fresh solution into someone else's FITS means clearing all of
/// them, not just the ones being rewritten: a leftover CDELT/CROTA pair, a
/// PC matrix or a SIP distortion polynomial would be read alongside the new
/// CD matrix and quietly move the solution.
pub fn is_wcs_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "WCSAXES", "WCSNAME", "EQUINOX", "EPOCH", "RADESYS", "RADECSYS", "LONPOLE", "LATPOLE",
        "IMAGEW", "IMAGEH", "A_ORDER", "B_ORDER", "AP_ORDER", "BP_ORDER", "A_DMAX", "B_DMAX",
    ];
    if EXACT.contains(&key) {
        return true;
    }
    // Axis-numbered keywords: CTYPE1, CRVAL2, CDELT1, ...
    const AXIS: &[&str] = &[
        "CTYPE", "CUNIT", "CRVAL", "CRPIX", "CDELT", "CROTA", "CRDER", "CSYER", "CNAME",
    ];
    if let Some(rest) = AXIS.iter().find_map(|p| key.strip_prefix(p)) {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    // i_j-indexed keywords: the CD and PC matrices, PV projection
    // parameters, and the SIP coefficients A_i_j / B_i_j / AP_i_j / BP_i_j.
    const INDEXED: &[&str] = &["CD", "PC", "PV", "AP_", "BP_", "A_", "B_"];
    if let Some(rest) = INDEXED.iter().find_map(|p| key.strip_prefix(p)) {
        let mut parts = rest.split('_');
        return matches!((parts.next(), parts.next(), parts.next()), (Some(i), Some(j), None)
            if !i.is_empty()
                && !j.is_empty()
                && i.bytes().chain(j.bytes()).all(|b| b.is_ascii_digit()));
    }
    false
}


/// 2x2 SVD: M = U * diag(s) * V^T with U, V rotations-or-reflections.
fn svd2x2(m: [[f64; 2]; 2]) -> ([[f64; 2]; 2], [f64; 2], [[f64; 2]; 2]) {
    let e = (m[0][0] + m[1][1]) / 2.0;
    let f = (m[0][0] - m[1][1]) / 2.0;
    let g = (m[1][0] + m[0][1]) / 2.0;
    let h = (m[1][0] - m[0][1]) / 2.0;
    let q = (e * e + h * h).sqrt();
    let r = (f * f + g * g).sqrt();
    let s1 = q + r;
    let s2 = q - r;
    let a1 = g.atan2(f);
    let a2 = h.atan2(e);
    // Derivation: with M = R(u)·diag(s)·R(v)^T, E,H carry u-v and F,G carry
    // u+v, so u = (a2+a1)/2 and v = (a1-a2)/2.
    let theta = (a1 - a2) / 2.0;
    let phi = (a2 + a1) / 2.0;
    // U = R(phi), V = R(theta), with s2 possibly negative folded into V.
    let u = [
        [phi.cos(), -phi.sin()],
        [phi.sin(), phi.cos()],
    ];
    let (s2abs, vsign) = if s2 < 0.0 { (-s2, -1.0) } else { (s2, 1.0) };
    let v = [
        [theta.cos(), -vsign * theta.sin()],
        [theta.sin(), vsign * theta.cos()],
    ];
    ([u[0], u[1]], [s1, s2abs], v)
}

/// Fit a TAN WCS from matched pairs: star unit vectors and field pixels.
/// Port of fit_tan_wcs_solve with tanin = NULL. Needs N >= 2 (N >= 3 for a
/// meaningful fit). Returns None on degenerate input.
pub fn fit_tan_wcs(xyz: &[[f64; 3]], pix: &[[f64; 2]]) -> Option<TanWcs> {
    let n = xyz.len();
    if n < 2 || pix.len() != n {
        return None;
    }
    // Field center of mass.
    let mut fcm = [0.0f64; 2];
    for p in pix {
        fcm[0] += p[0];
        fcm[1] += p[1];
    }
    fcm[0] /= n as f64;
    fcm[1] /= n as f64;

    // Star center of mass → tangent point.
    let mut scm = [0.0f64; 3];
    for s in xyz {
        scm[0] += s[0];
        scm[1] += s[1];
        scm[2] += s[2];
    }
    let scm = normalize(scm);

    // Project stars, subtract projected center of mass.
    let mut proj = Vec::with_capacity(n);
    let mut pcm = [0.0f64; 2];
    for s in xyz {
        let (x, y) = tan_project(*s, scm)?;
        pcm[0] += x;
        pcm[1] += y;
        proj.push([x, y]);
    }
    pcm[0] /= n as f64;
    pcm[1] /= n as f64;

    // Covariance H[j][k] = sum f_j * p_k  (f = centered field, p = centered proj)
    let mut cov = [[0.0f64; 2]; 2];
    let mut pvar = 0.0f64;
    let mut fvar = 0.0f64;
    for i in 0..n {
        let f = [pix[i][0] - fcm[0], pix[i][1] - fcm[1]];
        let p = [proj[i][0] - pcm[0], proj[i][1] - pcm[1]];
        for j in 0..2 {
            for k in 0..2 {
                cov[j][k] += p[k] * f[j];
            }
        }
        pvar += p[0] * p[0] + p[1] * p[1];
        fvar += f[0] * f[0] + f[1] * f[1];
    }
    if fvar <= 0.0 || pvar <= 0.0 || !cov.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    let (u, _s, v) = svd2x2(cov);
    // R = V * U^T
    let mut rot = [[0.0f64; 2]; 2];
    for j in 0..2 {
        for k in 0..2 {
            rot[j][k] = v[j][0] * u[k][0] + v[j][1] * u[k][1];
        }
    }
    let scale = (pvar / fvar).sqrt().to_degrees();
    let cd = [
        [rot[0][0] * scale, rot[0][1] * scale],
        [rot[1][0] * scale, rot[1][1] * scale],
    ];
    let (ra, dec) = xyz_to_radec(scm);
    Some(TanWcs {
        crval: [ra, dec],
        crpix: fcm,
        cd,
    })
}

/// Re-fit with the tangent point moved to a chosen reference pixel
/// (port of fit_tan_wcs_solve with tanin/crpix set). Orientation and parity
/// are conventionally reported at the image center; fitting there removes
/// the meridian-convergence offset a far-away tangent point introduces.
pub fn fit_tan_wcs_move(
    xyz: &[[f64; 3]],
    pix: &[[f64; 2]],
    crpix: [f64; 2],
    tanin: &TanWcs,
) -> Option<TanWcs> {
    let n = xyz.len();
    if n < 2 || pix.len() != n {
        return None;
    }
    let crxyz = tanin.pixel_to_xyz(crpix[0], crpix[1]);

    let mut fcm = [0.0f64; 2];
    for p in pix {
        fcm[0] += p[0];
        fcm[1] += p[1];
    }
    fcm[0] /= n as f64;
    fcm[1] /= n as f64;

    let mut proj = Vec::with_capacity(n);
    let mut pcm = [0.0f64; 2];
    for s in xyz {
        let (x, y) = tan_project(*s, crxyz)?;
        pcm[0] += x;
        pcm[1] += y;
        proj.push([x, y]);
    }
    pcm[0] /= n as f64;
    pcm[1] /= n as f64;

    let mut cov = [[0.0f64; 2]; 2];
    let mut pvar = 0.0f64;
    let mut fvar = 0.0f64;
    for i in 0..n {
        let f = [pix[i][0] - fcm[0], pix[i][1] - fcm[1]];
        let p = [proj[i][0] - pcm[0], proj[i][1] - pcm[1]];
        for j in 0..2 {
            for k in 0..2 {
                cov[j][k] += p[k] * f[j];
            }
        }
        pvar += p[0] * p[0] + p[1] * p[1];
        fvar += f[0] * f[0] + f[1] * f[1];
    }
    if fvar <= 0.0 || pvar <= 0.0 {
        return None;
    }
    let (u, _s, v) = svd2x2(cov);
    let mut rot = [[0.0f64; 2]; 2];
    for j in 0..2 {
        for k in 0..2 {
            rot[j][k] = v[j][0] * u[k][0] + v[j][1] * u[k][1];
        }
    }
    let scale = (pvar / fvar).sqrt().to_degrees();
    let cd = [
        [rot[0][0] * scale, rot[0][1] * scale],
        [rot[1][0] * scale, rot[1][1] * scale],
    ];
    // Temporary crval at the old mapping of crpix, then shift it so the
    // field center of mass lands where the projected stars say it should.
    let (tra, tdec) = tanin.pixel_to_radec(crpix[0], crpix[1]);
    let tmp = TanWcs {
        crval: [tra, tdec],
        crpix,
        cd,
    };
    let ix = cd[0][0] * (fcm[0] - crpix[0]) + cd[0][1] * (fcm[1] - crpix[1]);
    let iy = cd[1][0] * (fcm[0] - crpix[0]) + cd[1][1] * (fcm[1] - crpix[1]);
    let dx = pcm[0].to_degrees() - ix;
    let dy = pcm[1].to_degrees() - iy;
    let newcr = tan_unproject(
        dx.to_radians(),
        dy.to_radians(),
        radec_to_xyz(tmp.crval[0], tmp.crval[1]),
    );
    let (ra, dec) = xyz_to_radec(newcr);
    Some(TanWcs {
        crval: [ra, dec],
        crpix,
        cd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mul2(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
        let mut out = [[0.0; 2]; 2];
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    out[i][j] += a[i][k] * b[k][j];
                }
            }
        }
        out
    }

    #[test]
    fn svd_reconstructs() {
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 33) as f64 / u32::MAX as f64 - 0.5) * 4.0
        };
        for _ in 0..1000 {
            let m = [[rnd(), rnd()], [rnd(), rnd()]];
            let (u, s, v) = svd2x2(m);
            // orthonormality
            let utu = mul2([[u[0][0], u[1][0]], [u[0][1], u[1][1]]], u);
            let vtv = mul2([[v[0][0], v[1][0]], [v[0][1], v[1][1]]], v);
            for i in 0..2 {
                for j in 0..2 {
                    let idt = if i == j { 1.0 } else { 0.0 };
                    assert!((utu[i][j] - idt).abs() < 1e-9);
                    assert!((vtv[i][j] - idt).abs() < 1e-9);
                }
            }
            assert!(s[0] >= s[1] && s[1] >= -1e-12);
            // M = U S V^T
            let usv = mul2(
                mul2(u, [[s[0], 0.0], [0.0, s[1]]]),
                [[v[0][0], v[1][0]], [v[0][1], v[1][1]]],
            );
            for i in 0..2 {
                for j in 0..2 {
                    assert!(
                        (usv[i][j] - m[i][j]).abs() < 1e-9,
                        "M={m:?} USV={usv:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn project_unproject_roundtrip() {
        let r = radec_to_xyz(123.4, -33.2);
        for (dx, dy) in [(0.001, 0.002), (-0.01, 0.005), (0.0, 0.0)] {
            let s = tan_unproject(dx, dy, r);
            let (x, y) = tan_project(s, r).unwrap();
            assert!((x - dx).abs() < 1e-12 && (y - dy).abs() < 1e-12);
        }
    }

    #[test]
    fn fit_recovers_synthetic_wcs() {
        // Ground-truth WCS: 2 arcsec/px, rotated 30 deg, negative parity.
        let scale = 2.0 / 3600.0;
        let th: f64 = 30f64.to_radians();
        let truth = TanWcs {
            crval: [210.5, 54.3],
            crpix: [1000.0, 750.0],
            cd: [
                [-scale * th.cos(), scale * th.sin()],
                [scale * th.sin(), scale * th.cos()],
            ],
        };
        let mut xyz = Vec::new();
        let mut pix = Vec::new();
        let mut seed = 99u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as f64 / u32::MAX as f64
        };
        for _ in 0..50 {
            let p = [rnd() * 2000.0, rnd() * 1500.0];
            xyz.push(truth.pixel_to_xyz(p[0], p[1]));
            pix.push(p);
        }
        let coarse = fit_tan_wcs(&xyz, &pix).unwrap();
        // Move the tangent point to the same crpix as the truth WCS so
        // orientation/parity are directly comparable.
        let fit = fit_tan_wcs_move(&xyz, &pix, [1000.0, 750.0], &coarse).unwrap();
        // The reference algorithm (fit_tan_wcs_solve) ignores the projected
        // center-of-mass correction, so expect arcsecond-level, not exact,
        // agreement over a ~1 degree synthetic field.
        assert!(
            (fit.pixscale() - truth.pixscale()).abs() / truth.pixscale() < 1e-3,
            "pixscale {} vs {}",
            fit.pixscale(),
            truth.pixscale()
        );
        assert_eq!(fit.parity(), truth.parity());
        assert!(
            (fit.orientation() - truth.orientation()).abs() < 0.05,
            "orientation {} vs {}",
            fit.orientation(),
            truth.orientation()
        );
        // Center maps to the same sky position within ~10 arcsec.
        let (ra1, dec1) = truth.pixel_to_radec(1000.0, 750.0);
        let (ra2, dec2) = fit.pixel_to_radec(1000.0, 750.0);
        assert!((ra1 - ra2).abs() * dec1.to_radians().cos() < 3e-3);
        assert!((dec1 - dec2).abs() < 3e-3);
    }
    #[test]
    fn wcs_cards_round_trip_through_a_written_fits() {
        let wcs = TanWcs {
            crval: [83.94, -5.74],
            crpix: [999.5, 749.5],
            cd: [[-6.1e-4, 1.2e-5], [1.1e-5, 6.1e-4]],
        };
        let buf = fl_fits::write::header_only(&wcs.fits_cards(2000.0, 1500.0));
        let fits = fl_fits::Fits::parse(&buf).unwrap();
        let h = &fits.hdus[0].header;
        assert_eq!(h.get_str("CTYPE1"), Some("RA---TAN"));
        assert!((h.get_f64("CRVAL1").unwrap() - wcs.crval[0]).abs() < 1e-9);
        // 0-based internally, 1-based on disk.
        assert!((h.get_f64("CRPIX2").unwrap() - 750.5).abs() < 1e-9);
        assert!((h.get_f64("CD1_2").unwrap() - wcs.cd[0][1]).abs() < 1e-18);
        assert_eq!(h.get_f64("IMAGEW"), Some(2000.0));
    }

    #[test]
    fn wcs_keys_cover_the_conventions_we_replace() {
        for k in [
            "CRVAL1", "CRPIX2", "CTYPE1", "CUNIT2", "CDELT1", "CROTA2", "CD1_1", "PC2_1", "PV2_3",
            "A_2_0", "BP_0_1", "A_ORDER", "EQUINOX", "RADESYS", "LONPOLE", "IMAGEW",
        ] {
            assert!(is_wcs_key(k), "{k} should be a WCS keyword");
        }
        for k in [
            "SIMPLE", "BITPIX", "NAXIS1", "OBJECT", "EXPTIME", "BZERO", "DATE-OBS", "CCD-TEMP",
            "CD", "COMMENT", "GAIN", "AIRMASS",
        ] {
            assert!(!is_wcs_key(k), "{k} should be preserved");
        }
    }
}
