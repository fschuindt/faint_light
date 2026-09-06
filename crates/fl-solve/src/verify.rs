//! WCS hypothesis verification: project catalog stars into the image and
//! score the alignment with detected stars via a log-odds ratio
//! (a simplified port of astrometry.net's verify.c model).

use fl_extract::Star;
use fl_index::IndexFile;

use crate::tanwcs::{dist2, rad_to_dist2, TanWcs};

#[derive(Debug, Clone)]
pub struct VerifyParams {
    /// Positional uncertainty (pixels) for a true match.
    pub sigma_px: f64,
    /// Prior probability that a bright field star is a distractor
    /// (not present in the index).
    pub distractor: f64,
    /// Use at most this many field stars (brightest first).
    pub max_field: usize,
    /// Log-odds needed to accept.
    pub accept_logodds: f64,
    /// Minimum matched pairs to accept.
    pub min_matches: usize,
}

impl Default for VerifyParams {
    fn default() -> Self {
        VerifyParams {
            sigma_px: 2.0,
            distractor: 0.25,
            max_field: 80,
            accept_logodds: (1e9f64).ln(),
            min_matches: 6,
        }
    }
}

#[derive(Debug)]
pub struct VerifyOutcome {
    pub logodds: f64,
    pub nmatch: usize,
    /// Matched (star xyz, field pixel) pairs for WCS refitting.
    pub pairs: Vec<([f64; 3], [f64; 2])>,
}

impl VerifyOutcome {
    pub fn accepted(&self, p: &VerifyParams) -> bool {
        self.logodds >= p.accept_logodds && self.nmatch >= p.min_matches
    }
}

/// Score `wcs` against the extracted field stars using `index`'s star tree.
pub fn verify_wcs(
    wcs: &TanWcs,
    index: &IndexFile,
    field: &[Star],
    w: f64,
    h: f64,
    p: &VerifyParams,
) -> VerifyOutcome {
    let center = wcs.pixel_to_xyz((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    // Angular radius of the field: center to corner, plus 5% slack.
    let corner = wcs.pixel_to_xyz(-0.5, -0.5);
    let radius2 = rad_to_dist2(crate::tanwcs::dist2_to_rad(dist2(center, corner)) * 1.05);

    let mut hits: Vec<(u32, f64)> = Vec::new();
    index.stars().range_search(&center, radius2, &mut hits);

    // Project index stars into pixel space; keep those on the image
    // (small margin so near-edge matches still count).
    let margin = 4.0 * p.sigma_px;
    let mut proj: Vec<(u32, [f64; 3], f64, f64)> = Vec::with_capacity(hits.len());
    for (id, _) in hits {
        let xyz = index.star_xyz(id);
        if let Some((px, py)) = wcs.xyz_to_pixel(xyz) {
            if px >= -margin && px < w + margin && py >= -margin && py < h + margin {
                proj.push((id, xyz, px, py));
            }
        }
    }

    let nindex = proj.len();
    if nindex == 0 {
        return VerifyOutcome { logodds: f64::NEG_INFINITY, nmatch: 0, pairs: Vec::new() };
    }

    let bg = 1.0 / (w * h);
    let sigma2 = p.sigma_px * p.sigma_px;
    let gauss_norm = 1.0 / (2.0 * std::f64::consts::PI * sigma2);
    // A match may pair with any of the index stars in the field; dividing the
    // foreground by nindex keeps the model honest in dense fields.
    let fg_scale = (1.0 - p.distractor) * gauss_norm / nindex as f64;
    let match_r2 = (5.0 * p.sigma_px) * (5.0 * p.sigma_px);

    let mut used = vec![false; nindex];
    let mut logodds = 0.0f64;
    let mut nmatch = 0usize;
    let mut pairs = Vec::new();

    for star in field.iter().take(p.max_field) {
        // Nearest unused projected index star.
        let mut best = usize::MAX;
        let mut best_d2 = match_r2;
        for (j, &(_, _, px, py)) in proj.iter().enumerate() {
            if used[j] {
                continue;
            }
            let d2 = (px - star.x).powi(2) + (py - star.y).powi(2);
            if d2 < best_d2 {
                best_d2 = d2;
                best = j;
            }
        }
        let odds = if best != usize::MAX {
            let fg = fg_scale * (-best_d2 / (2.0 * sigma2)).exp();
            fg / bg + p.distractor
        } else {
            p.distractor
        };
        // Cap per-star contribution to keep one lucky coincidence from
        // dominating (verify.c caps similarly via its model).
        logodds += odds.ln().min((1e5f64).ln());
        if best != usize::MAX && odds > 1.0 {
            used[best] = true;
            nmatch += 1;
            pairs.push((proj[best].1, [star.x, star.y]));
        }
        // Early bail: hopeless hypothesis.
        if logodds < -(1e6f64).ln() {
            break;
        }
    }

    VerifyOutcome { logodds, nmatch, pairs }
}
