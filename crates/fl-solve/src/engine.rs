//! The solve engine: scale-band ladder, quad enumeration (mirroring
//! astrometry.net's solver_run ordering: brightest stars first, each quad
//! tried exactly once), code search, hypothesis fitting and verification,
//! plus the warm-start fast paths that make repeat solves near the last
//! field cheap.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fl_extract::Star;
use fl_index::cache::IndexCache;
use fl_index::IndexFile;

use crate::codes::{for_each_code, CodeInvariants, PQuad, Parity};
use crate::tanwcs::{dist2, dist2_to_rad, fit_tan_wcs, rad_to_dist2, TanWcs};
use crate::verify::{verify_wcs, VerifyOutcome, VerifyParams};
use crate::{Calibration, SolveError, SolveHints};

const ARCSEC_PER_RAD: f64 = 180.0 * 3600.0 / std::f64::consts::PI;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub index_dir: PathBuf,
    pub cache_gb: f64,
    /// Code-space match tolerance (astrometry.net codetol).
    pub codetol: f64,
    /// Rig scale prior (arcsec/px) from server config, if any.
    pub default_scale_lo: Option<f64>,
    pub default_scale_hi: Option<f64>,
    /// Widest scale band tried in blind fallback.
    pub wide_scale_lo: f64,
    pub wide_scale_hi: f64,
    /// Use at most this many stars for quad building.
    pub max_quad_stars: usize,
    pub solve_timeout: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            index_dir: PathBuf::from("./indexes"),
            cache_gb: 2.0,
            codetol: 0.01,
            default_scale_lo: None,
            default_scale_hi: None,
            wide_scale_lo: 0.1,
            wide_scale_hi: 300.0,
            max_quad_stars: 64,
            solve_timeout: Duration::from_secs(300),
        }
    }
}

#[derive(Clone)]
struct LastSolve {
    wcs: TanWcs,
    pixscale: f64,
    index: usize,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    lo: f64, // arcsec/px
    hi: f64,
}

/// An index active in the current scale band, with the pixel-length window
/// its quads' AB backbones can span.
struct Active<'a> {
    index: &'a IndexFile,
    ab_min_px: f64,
    ab_max_px: f64,
}

type Group<'a> = (usize, CodeInvariants, Vec<Active<'a>>);

/// How many recent solves to remember (covers a user alternating between a
/// couple of rigs/cameras without falling back to blind search).
const WARM_SLOTS: usize = 4;

pub struct Engine {
    cache: IndexCache,
    cfg: EngineConfig,
    /// Most-recent-first list of past solves.
    warm: Mutex<Vec<LastSolve>>,
}

/// A fully-scored solution.
#[derive(Debug, Clone)]
pub struct Solution {
    pub wcs: TanWcs,
    pub calibration: Calibration,
    pub logodds: f64,
    pub nmatch: usize,
    pub index_id: i64,
    /// Image dimensions in pixels (for width/height reporting and WCS files).
    pub width: f64,
    pub height: f64,
}

impl Engine {
    pub fn new(cfg: EngineConfig) -> Result<Engine, SolveError> {
        let cache = IndexCache::open_dir(&cfg.index_dir, cfg.cache_gb)
            .map_err(|e| SolveError::Setup(e.to_string()))?;
        if cache.indexes.is_empty() {
            return Err(SolveError::Setup(format!(
                "no index files found in {}",
                cfg.index_dir.display()
            )));
        }
        let engine = Engine {
            cache,
            cfg,
            warm: Mutex::new(Vec::new()),
        };
        engine.prewarm();
        Ok(engine)
    }

    pub fn index_count(&self) -> usize {
        self.cache.indexes.len()
    }

    /// Prefetch indexes into RAM, preferring those relevant to the
    /// configured (or last-solved) pixel scale, up to the cache budget.
    fn prewarm(&self) {
        let band = self
            .warm
            .lock()
            .unwrap()
            .first()
            .map(|lw| Band { lo: lw.pixscale * 0.8, hi: lw.pixscale * 1.25 })
            .or_else(|| self.default_band());
        let relevant = |i: &IndexFile| -> bool {
            let Some(b) = band else { return true };
            // Quads an image of 500..8000 px could contain at this scale.
            let lo_rad = b.lo / ARCSEC_PER_RAD * 50.0;
            let hi_rad = b.hi / ARCSEC_PER_RAD * 8000.0;
            i.meta.scale_upper_rad >= lo_rad && i.meta.scale_lower_rad <= hi_rad
        };
        let mut order: Vec<usize> = (0..self.cache.indexes.len()).collect();
        order.sort_by_key(|&i| !relevant(&self.cache.indexes[i]) as u8);
        self.cache.prewarm(order.into_iter());
    }

    fn default_band(&self) -> Option<Band> {
        match (self.cfg.default_scale_lo, self.cfg.default_scale_hi) {
            (Some(lo), Some(hi)) if lo > 0.0 && hi > lo => Some(Band { lo, hi }),
            _ => None,
        }
    }

    /// Solve from raw upload bytes.
    pub fn solve_image(&self, bytes: &[u8], hints: &SolveHints) -> Result<Solution, SolveError> {
        let t0 = Instant::now();
        let img = fl_extract::decode(bytes).map_err(|e| SolveError::Decode(e.to_string()))?;
        let mut params = fl_extract::ExtractParams::default();
        if let Some(d) = hints.downsample {
            params.downsample = d.max(1);
        } else if img.w * img.h < 1_500_000 {
            params.downsample = 1;
        }
        let stars = fl_extract::extract_stars(&img, &params);
        tracing::info!(
            w = img.w,
            h = img.h,
            stars = stars.len(),
            ms = t0.elapsed().as_millis() as u64,
            "extracted"
        );
        if stars.len() < 4 {
            return Err(SolveError::NoSolution);
        }
        let mut hints = hints.clone();
        if let Some((wlo, whi)) = hints.width_deg {
            // Field-width hints resolve to arcsec/px now that width is known.
            hints.scale_lo = Some(wlo * 3600.0 / img.w as f64);
            hints.scale_hi = Some(whi * 3600.0 / img.w as f64);
        }
        let mut sol = self.solve_field(&stars, img.w as f64, img.h as f64, &hints)?;
        // nova convention: for JPEG/PNG-like inputs (not FITS) the reported
        // orientation is flipped up/down (net/models.py get_orientation).
        if !bytes.starts_with(b"SIMPLE  =") {
            sol.calibration.orientation =
                (180.0 - sol.calibration.orientation).rem_euclid(360.0);
        }
        Ok(sol)
    }

    /// Solve from an extracted star list.
    pub fn solve_field(
        &self,
        stars: &[Star],
        w: f64,
        h: f64,
        hints: &SolveHints,
    ) -> Result<Solution, SolveError> {
        let deadline = Instant::now() + self.cfg.solve_timeout;

        // ---- Tier 0: re-verify the last WCS against the new star field.
        if let Some(sol) = self.try_warm(stars, w, h, hints) {
            tracing::info!(logodds = sol.logodds, nmatch = sol.nmatch, "tier0 warm hit");
            self.store_warm(&sol, w, h);
            return Ok(sol);
        }

        // ---- Scale-band ladder.
        let hint_band = match (hints.scale_lo, hints.scale_hi) {
            (Some(lo), Some(hi)) if hi > lo => Some(Band { lo, hi }),
            _ => None,
        };
        let warm_bands: Vec<Band> = self
            .warm
            .lock()
            .unwrap()
            .iter()
            .map(|lw| Band {
                lo: lw.pixscale * 0.85,
                hi: lw.pixscale * 1.18,
            })
            .collect();
        let mut bands: Vec<Band> = Vec::new();
        if let Some(b) = hint_band {
            bands.push(b);
        } else {
            bands.extend(warm_bands);
            if let Some(b) = self.default_band() {
                bands.push(b);
            }
            bands.push(Band {
                lo: self.cfg.wide_scale_lo,
                hi: self.cfg.wide_scale_hi,
            });
        }
        // Drop bands fully covered by an earlier one.
        let mut seen: Vec<Band> = Vec::new();
        bands.retain(|b| {
            let dup = seen.iter().any(|s| s.lo <= b.lo && s.hi >= b.hi);
            if !dup {
                seen.push(*b);
            }
            !dup
        });

        let pos = match (hints.center_ra, hints.center_dec) {
            (Some(ra), Some(dec)) => {
                let r = hints.radius.unwrap_or(15.0).to_radians();
                Some((crate::tanwcs::radec_to_xyz(ra, dec), r))
            }
            _ => None,
        };

        let nbands = bands.len();
        for (i, band) in bands.iter().enumerate() {
            let t0 = Instant::now();
            // Guess bands (warm scale, configured rig scale) are cheap when
            // right and expensive when stale — give each a time slice and
            // save the bulk of the budget for the final wide fallback.
            let attempt_deadline = if i + 1 < nbands {
                deadline.min(Instant::now() + Duration::from_secs(5))
            } else {
                deadline
            };
            tracing::info!(lo = band.lo, hi = band.hi, attempt = i, "quad search");
            if let Some(sol) =
                self.quad_search(stars, w, h, *band, pos, hints.parity, attempt_deadline)
            {
                tracing::info!(
                    logodds = sol.logodds,
                    nmatch = sol.nmatch,
                    index = sol.index_id,
                    ms = t0.elapsed().as_millis() as u64,
                    "solved"
                );
                self.store_warm(&sol, w, h);
                self.prewarm();
                return Ok(sol);
            }
            if Instant::now() > deadline {
                return Err(SolveError::Timeout);
            }
        }
        Err(SolveError::NoSolution)
    }

    fn store_warm(&self, sol: &Solution, w: f64, h: f64) {
        let index = self
            .cache
            .indexes
            .iter()
            .position(|i| i.meta.index_id == sol.index_id)
            .unwrap_or(0);
        let mut warm = self.warm.lock().unwrap();
        // One slot per rig: same dimensions and similar pixel scale.
        warm.retain(|lw| {
            !(lw.width == w
                && lw.height == h
                && (lw.pixscale / sol.calibration.pixscale - 1.0).abs() < 0.10)
        });
        warm.insert(
            0,
            LastSolve {
                wcs: sol.wcs,
                pixscale: sol.calibration.pixscale,
                index,
                width: w,
                height: h,
            },
        );
        warm.truncate(WARM_SLOTS);
    }

    /// Tier 0: the mount barely moved (dither, re-center, next sub). Verify
    /// the previous WCS directly with a generous match radius, refit from
    /// the matches, then require the refit to pass strict verification.
    fn try_warm(&self, stars: &[Star], w: f64, h: f64, hints: &SolveHints) -> Option<Solution> {
        let candidates: Vec<LastSolve> = self.warm.lock().unwrap().clone();
        for last in candidates {
            // Image geometry changed → pixel WCS is not transferable.
            if (last.width - w).abs() > 0.5 || (last.height - h).abs() > 0.5 {
                continue;
            }
            // Contradicting client hints disable the shortcut.
            if let (Some(lo), Some(hi)) = (hints.scale_lo, hints.scale_hi) {
                if last.pixscale < lo || last.pixscale > hi {
                    continue;
                }
            }
            let index = &self.cache.indexes[last.index];
            let loose = VerifyParams {
                sigma_px: 10.0,
                min_matches: 8,
                accept_logodds: (1e6f64).ln(),
                ..Default::default()
            };
            let out = verify_wcs(&last.wcs, index, stars, w, h, &loose);
            if out.pairs.len() < 6 {
                continue;
            }
            if let Some(sol) = self.refit_and_accept(&last.wcs, &out, index, stars, w, h) {
                return Some(sol);
            }
        }
        None
    }

    /// Refit the WCS from verified matches and re-verify strictly.
    fn refit_and_accept(
        &self,
        wcs0: &TanWcs,
        out0: &VerifyOutcome,
        index: &IndexFile,
        stars: &[Star],
        w: f64,
        h: f64,
    ) -> Option<Solution> {
        let (xyz, pix): (Vec<[f64; 3]>, Vec<[f64; 2]>) = out0.pairs.iter().cloned().unzip();
        let refit = fit_tan_wcs(&xyz, &pix).unwrap_or(*wcs0);
        let strict = VerifyParams {
            sigma_px: verify_sigma(refit.pixscale()),
            ..Default::default()
        };
        let out = verify_wcs(&refit, index, stars, w, h, &strict);
        if !out.accepted(&strict) {
            return None;
        }
        // One more refinement round with the tighter matches, then move the
        // tangent point to the image center: orientation/parity are reported
        // there (nova convention), which removes meridian-convergence skew.
        let (xyz2, pix2): (Vec<[f64; 3]>, Vec<[f64; 2]>) = out.pairs.iter().cloned().unzip();
        let mut final_wcs = if xyz2.len() >= 6 {
            fit_tan_wcs(&xyz2, &pix2).unwrap_or(refit)
        } else {
            refit
        };
        let center = [(w - 1.0) / 2.0, (h - 1.0) / 2.0];
        if let Some(moved) =
            crate::tanwcs::fit_tan_wcs_move(&xyz2, &pix2, center, &final_wcs)
        {
            final_wcs = moved;
        }
        Some(build_solution(final_wcs, out, index, w, h))
    }

    /// The main quad-matching search over one scale band.
    #[allow(clippy::too_many_arguments)]
    fn quad_search(
        &self,
        stars: &[Star],
        w: f64,
        h: f64,
        band: Band,
        pos: Option<([f64; 3], f64)>,
        parity: Parity,
        deadline: Instant,
    ) -> Option<Solution> {
        let diag = (w * w + h * h).sqrt();
        let funits_lo_rad = band.lo / ARCSEC_PER_RAD;
        let funits_hi_rad = band.hi / ARCSEC_PER_RAD;

        let mut groups: Vec<Group> = Vec::new();
        for index in self.cache.indexes.iter() {
            let m = &index.meta;
            let ab_min = (m.scale_lower_rad / funits_hi_rad).max(8.0);
            let ab_max = (m.scale_upper_rad / funits_lo_rad).min(diag);
            if ab_min >= ab_max {
                continue;
            }
            let inv = CodeInvariants {
                cx_le_dx: m.cx_le_dx,
                meanx_le_half: m.meanx_le_half,
            };
            let entry = Active {
                index,
                ab_min_px: ab_min,
                ab_max_px: ab_max,
            };
            match groups.iter_mut().find(|(dq, gi, _)| {
                *dq == m.dimquads
                    && gi.cx_le_dx == inv.cx_le_dx
                    && gi.meanx_le_half == inv.meanx_le_half
            }) {
                Some((_, _, v)) => v.push(entry),
                None => groups.push((m.dimquads, inv, vec![entry])),
            }
        }
        if groups.is_empty() {
            return None;
        }
        let min_ab2 = groups
            .iter()
            .flat_map(|(_, _, v)| v.iter().map(|a| a.ab_min_px * a.ab_min_px))
            .fold(f64::INFINITY, f64::min);
        let max_ab2 = groups
            .iter()
            .flat_map(|(_, _, v)| v.iter().map(|a| a.ab_max_px * a.ab_max_px))
            .fold(0.0f64, f64::max);

        let n = stars.len().min(self.cfg.max_quad_stars);
        if n < 3 {
            return None;
        }
        let xy: Vec<(f64, f64)> = stars[..n].iter().map(|s| (s.x, s.y)).collect();

        // pquads indexed by pair (a < b): b*(b-1)/2 + a.
        let mut pquads: Vec<Option<PQuad>> = Vec::new();
        pquads.resize_with(n * (n - 1) / 2, || None);
        let pair = |a: usize, b: usize| b * (b - 1) / 2 + a;

        let tol2 = self.cfg.codetol * self.cfg.codetol;

        for newpoint in 1..n {
            if Instant::now() > deadline {
                return None;
            }
            // (1) New backbones: B = newpoint, A < newpoint.
            for a in 0..newpoint {
                let mut pq = PQuad::new(a, newpoint, xy[a], xy[newpoint]);
                if pq.scale2 < min_ab2 || pq.scale2 > max_ab2 {
                    pquads[pair(a, newpoint)] = Some(pq);
                    continue;
                }
                for m in 0..newpoint {
                    if m != a {
                        pq.add_member(m, xy[m], self.cfg.codetol);
                    }
                }
                if let Some(sol) =
                    self.try_pquad(&pq, None, &groups, stars, &xy, w, h, band, pos, parity, tol2)
                {
                    return Some(sol);
                }
                pquads[pair(a, newpoint)] = Some(pq);
            }
            // (2) Existing backbones gain `newpoint` as a member.
            for b in 1..newpoint {
                if Instant::now() > deadline {
                    return None;
                }
                for a in 0..b {
                    let Some(pq) = pquads[pair(a, b)].as_mut() else {
                        continue;
                    };
                    if pq.scale2 < min_ab2 || pq.scale2 > max_ab2 {
                        continue;
                    }
                    if !pq.add_member(newpoint, xy[newpoint], self.cfg.codetol) {
                        continue;
                    }
                    let pq = pquads[pair(a, b)].as_ref().unwrap();
                    if let Some(sol) = self.try_pquad(
                        pq,
                        Some(newpoint),
                        &groups,
                        stars,
                        &xy,
                        w,
                        h,
                        band,
                        pos,
                        parity,
                        tol2,
                    ) {
                        return Some(sol);
                    }
                }
            }
            if Instant::now() > deadline {
                return None;
            }
        }
        None
    }

    /// Try all member combinations of one backbone (optionally required to
    /// include `must`), against every active index group.
    #[allow(clippy::too_many_arguments)]
    fn try_pquad(
        &self,
        pq: &PQuad,
        must: Option<usize>,
        groups: &[Group],
        stars: &[Star],
        xy: &[(f64, f64)],
        w: f64,
        h: f64,
        band: Band,
        pos: Option<([f64; 3], f64)>,
        parity: Parity,
        tol2: f64,
    ) -> Option<Solution> {
        let ab_px = pq.scale2.sqrt();
        let must_pos = must.map(|m| pq.inbox.iter().position(|&x| x == m).unwrap());
        let mut hits: Vec<(u32, f64)> = Vec::new();
        for (dq, inv, actives) in groups {
            let k = dq - 2;
            if pq.inbox.len() < k {
                continue;
            }
            let in_window: Vec<&Active> = actives
                .iter()
                .filter(|a| ab_px >= a.ab_min_px && ab_px <= a.ab_max_px)
                .collect();
            if in_window.is_empty() {
                continue;
            }
            let mut combo = vec![0usize; k];
            let mut found: Option<Solution> = None;
            combos(pq.inbox.len(), k, must_pos, &mut combo, &mut |sel| {
                let members: Vec<usize> = sel.iter().map(|&i| pq.inbox[i]).collect();
                let mxy: Vec<(f64, f64)> = sel.iter().map(|&i| pq.inbox_xy[i]).collect();
                for_each_code(
                    pq,
                    &members,
                    &mxy,
                    *inv,
                    self.cfg.codetol,
                    parity,
                    &mut |order, code, _flip| {
                        for act in &in_window {
                            hits.clear();
                            act.index.codes().range_search(code, tol2, &mut hits);
                            for &(quad_id, _) in hits.iter() {
                                if let Some(sol) = self.resolve_hit(
                                    act.index, quad_id, order, stars, xy, w, h, band, pos,
                                ) {
                                    found = Some(sol);
                                    return true;
                                }
                            }
                        }
                        false
                    },
                )
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// A code-space hit: check position/scale gates, fit a WCS from the quad,
    /// verify against the catalog.
    #[allow(clippy::too_many_arguments)]
    fn resolve_hit(
        &self,
        index: &IndexFile,
        quad_id: u32,
        order: &[usize],
        stars: &[Star],
        xy: &[(f64, f64)],
        w: f64,
        h: f64,
        band: Band,
        pos: Option<([f64; 3], f64)>,
    ) -> Option<Solution> {
        let (qstars, dq) = index.quad_stars(quad_id as usize);
        debug_assert_eq!(dq, order.len());
        let mut sxyz = [[0f64; 3]; crate::codes::DQMAX];
        for i in 0..dq {
            sxyz[i] = index.star_xyz(qstars[i]);
            if let Some((center, radius)) = pos {
                let max_r = radius + 2.0 * index.meta.scale_upper_rad;
                if dist2(sxyz[i], center) > rad_to_dist2(max_r) {
                    return None;
                }
            }
        }
        // Quick scale gate from the AB pair alone (20% fudge).
        let ab_ang = dist2_to_rad(dist2(sxyz[0], sxyz[1]));
        let ab_px = {
            let (ax, ay) = xy[order[0]];
            let (bx, by) = xy[order[1]];
            ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt()
        };
        let radppx = ab_ang / ab_px;
        if radppx < band.lo / ARCSEC_PER_RAD / 1.2 || radppx > band.hi / ARCSEC_PER_RAD * 1.2 {
            return None;
        }

        let pix: Vec<[f64; 2]> = order.iter().map(|&i| [xy[i].0, xy[i].1]).collect();
        let wcs = fit_tan_wcs(&sxyz[..dq], &pix)?;
        let pixscale = wcs.pixscale();
        if pixscale < band.lo || pixscale > band.hi {
            return None;
        }

        let strict = VerifyParams {
            sigma_px: verify_sigma(pixscale),
            ..Default::default()
        };
        let out = verify_wcs(&wcs, index, stars, w, h, &strict);
        if !out.accepted(&strict) {
            return None;
        }
        self.refit_and_accept(&wcs, &out, index, stars, w, h)
    }
}

/// Verification sigma: extraction jitter plus index astrometric jitter
/// (~1 arcsec) expressed in pixels.
fn verify_sigma(pixscale: f64) -> f64 {
    let jitter_px = 1.0 / pixscale;
    (2.0f64.powi(2) + jitter_px * jitter_px).sqrt()
}

fn build_solution(wcs: TanWcs, out: VerifyOutcome, index: &IndexFile, w: f64, h: f64) -> Solution {
    let (cx, cy) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    let (ra, dec) = wcs.pixel_to_radec(cx, cy);
    let center = wcs.pixel_to_xyz(cx, cy);
    let corner = wcs.pixel_to_xyz(-0.5, -0.5);
    let radius = dist2_to_rad(dist2(center, corner)).to_degrees();
    Solution {
        calibration: Calibration {
            ra,
            dec,
            radius,
            pixscale: wcs.pixscale(),
            orientation: wcs.orientation(),
            parity: wcs.parity(),
        },
        wcs,
        logodds: out.logodds,
        nmatch: out.nmatch,
        index_id: index.meta.index_id,
        width: w,
        height: h,
    }
}

/// Enumerate k-combinations of 0..n (ascending); combos not containing
/// `must` (when set) are skipped. `cb` returning true stops enumeration.
fn combos(
    n: usize,
    k: usize,
    must: Option<usize>,
    buf: &mut [usize],
    cb: &mut impl FnMut(&[usize]) -> bool,
) -> bool {
    fn rec(
        n: usize,
        k: usize,
        start: usize,
        depth: usize,
        must: Option<usize>,
        buf: &mut [usize],
        cb: &mut impl FnMut(&[usize]) -> bool,
    ) -> bool {
        if depth == k {
            if let Some(m) = must {
                if !buf[..k].contains(&m) {
                    return false;
                }
            }
            return cb(&buf[..k]);
        }
        for i in start..n {
            buf[depth] = i;
            if rec(n, k, i + 1, depth + 1, must, buf, cb) {
                return true;
            }
        }
        false
    }
    rec(n, k, 0, 0, must, buf, cb)
}
