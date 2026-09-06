//! Convention-pinning tests against real index files:
//!
//! 1. Code round-trip: project a real index quad's stars through a synthetic
//!    WCS, run the field-code enumeration, and require a candidate within
//!    codetol of the code stored in the index. If A/B ordering, the circle
//!    frame, parity handling, or invariant enforcement diverge from
//!    build-index, this fails for every quad (silent-total-failure guard).
//!
//! 2. Synthetic solve: render a fake star field from index stars via a known
//!    WCS and require the engine to recover it.
//!
//! Gated on FAINT_LIGHT_TEST_INDEX_DIR.

use std::path::PathBuf;
use std::time::Duration;

use fl_extract::Star;
use fl_index::IndexFile;
use fl_solve::codes::{for_each_code, CodeInvariants, PQuad, Parity};
use fl_solve::tanwcs::{radec_to_xyz, xyz_to_radec, TanWcs};
use fl_solve::{Engine, EngineConfig, SolveHints};

fn index_dir() -> Option<PathBuf> {
    std::env::var("FAINT_LIGHT_TEST_INDEX_DIR").ok().map(Into::into)
}

struct XorShift(u64);
impl XorShift {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Build a WCS centered on the quad with north-up, given pixel scale.
fn wcs_for(center: [f64; 3], pixscale_arcsec: f64, w: f64, h: f64) -> TanWcs {
    let (ra, dec) = xyz_to_radec(center);
    let s = pixscale_arcsec / 3600.0;
    TanWcs {
        crval: [ra, dec],
        crpix: [(w - 1.0) / 2.0, (h - 1.0) / 2.0],
        // Negative-parity (normal astronomical) orientation 0.
        cd: [[-s, 0.0], [0.0, s]],
    }
}

#[test]
fn index_quad_code_roundtrip() {
    let Some(dir) = index_dir() else {
        eprintln!("FAINT_LIGHT_TEST_INDEX_DIR not set; skipping");
        return;
    };
    let mut rng = XorShift(0xBADC0FFEE);
    for name in ["index-4110.fits", "index-4112.fits", "index-4119.fits"] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let idx = IndexFile::open(&path).expect("open index");
        let dq = idx.meta.dimquads;
        let dc = 2 * (dq - 2);
        let codetol = 0.01;
        let inv = CodeInvariants {
            cx_le_dx: idx.meta.cx_le_dx,
            meanx_le_half: idx.meta.meanx_le_half,
        };

        let mut tried = 0;
        let mut ok = 0;
        for _ in 0..200 {
            let q = (rng.next_f64() * idx.meta.nquads as f64) as usize % idx.meta.nquads;
            let (qstars, _) = idx.quad_stars(q);
            let xyz: Vec<[f64; 3]> = qstars[..dq].iter().map(|&s| idx.star_xyz(s)).collect();
            // Quad centroid → WCS; scale so the quad spans ~600 px.
            let mut c = [0.0; 3];
            for s in &xyz {
                for d in 0..3 {
                    c[d] += s[d];
                }
            }
            let n = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            for d in &mut c {
                *d /= n;
            }
            let quad_rad = idx.meta.scale_upper_rad;
            let pixscale = quad_rad.to_degrees() * 3600.0 / 600.0;
            let (w, h) = (2000.0, 2000.0);
            let wcs = wcs_for(c, pixscale, w, h);
            let pix: Option<Vec<(f64, f64)>> =
                xyz.iter().map(|&s| wcs.xyz_to_pixel(s)).collect();
            let Some(pix) = pix else { continue };
            tried += 1;

            // Stored code for this quad.
            let mut stored = vec![0.0; dc];
            idx.codes().point(q, &mut stored);

            // Enumerate field codes over ALL AB pairs of the quad members
            // (the solver would find this backbone eventually).
            let mut hit = false;
            'outer: for a in 0..dq {
                for b in 0..dq {
                    if a == b {
                        continue;
                    }
                    let pq = PQuad::new(a, b, pix[a], pix[b]);
                    let members: Vec<usize> = (0..dq).filter(|&m| m != a && m != b).collect();
                    let mxy: Vec<(f64, f64)> =
                        members.iter().map(|&m| pq.code_xy(pix[m])).collect();
                    if !mxy.iter().all(|&c| PQuad::in_circle(c, codetol)) {
                        continue;
                    }
                    let found = for_each_code(
                        &pq,
                        &members,
                        &mxy,
                        inv,
                        codetol,
                        Parity::Both,
                        &mut |_order, code, _flip| {
                            let d2: f64 = code
                                .iter()
                                .zip(&stored)
                                .map(|(a, b)| (a - b) * (a - b))
                                .sum();
                            d2 < codetol * codetol
                        },
                    );
                    if found {
                        hit = true;
                        break 'outer;
                    }
                }
            }
            if hit {
                ok += 1;
            }
        }
        eprintln!("{name}: {ok}/{tried} quads round-trip");
        assert!(tried > 100, "{name}: too few testable quads");
        // Gnomonic-vs-pixel projection differences can push a rare boundary
        // quad outside codetol — substantially so for the ultra-wide indexes
        // whose quads span tens of degrees. A convention mismatch would
        // score ~0%, so the bar only needs to reject that.
        let bar = if idx.meta.scale_upper_rad > 0.15 { 0.90 } else { 0.97 };
        assert!(
            ok as f64 >= tried as f64 * bar,
            "{name}: only {ok}/{tried} quads round-tripped — code convention mismatch?"
        );
    }
}

#[test]
fn synthetic_field_solves() {
    let Some(dir) = index_dir() else {
        eprintln!("FAINT_LIGHT_TEST_INDEX_DIR not set; skipping");
        return;
    };
    let idx = IndexFile::open(&dir.join("index-4110.fits")).expect("open 4110");

    // Truth: a 2.2 deg field near a random-ish sky point, 4 arcsec/px.
    let truth_center = radec_to_xyz(83.0, 22.0);
    let (w, h) = (2000.0, 1500.0);
    let truth = wcs_for(truth_center, 4.0, w, h);

    // Collect index stars inside the field.
    let mut hits = Vec::new();
    idx.stars()
        .range_search(&truth_center, fl_solve::tanwcs::rad_to_dist2(0.03), &mut hits);
    let mut stars: Vec<Star> = Vec::new();
    let mut rng = XorShift(0x1234_5678);
    for (id, _) in hits {
        let xyz = idx.star_xyz(id);
        if let Some((px, py)) = truth.xyz_to_pixel(xyz) {
            if px >= 0.0 && px < w && py >= 0.0 && py < h {
                // ~0.3 px centroid noise
                stars.push(Star {
                    x: px + (rng.next_f64() - 0.5) * 0.6,
                    y: py + (rng.next_f64() - 0.5) * 0.6,
                    flux: 1000.0 - stars.len() as f32,
                });
            }
        }
    }
    eprintln!("synthetic field: {} stars", stars.len());
    assert!(stars.len() >= 15, "field too sparse for the test");

    let engine = Engine::new(EngineConfig {
        index_dir: dir.clone(),
        solve_timeout: Duration::from_secs(60),
        ..Default::default()
    })
    .expect("engine");

    let sol = engine
        .solve_field(&stars, w, h, &SolveHints::default())
        .expect("synthetic field should solve");
    let cal = &sol.calibration;
    eprintln!(
        "solved: ra={:.4} dec={:.4} scale={:.3} orient={:.2} parity={} logodds={:.1} nmatch={}",
        cal.ra, cal.dec, cal.pixscale, cal.orientation, cal.parity, sol.logodds, sol.nmatch
    );
    assert!((cal.ra - 83.0).abs() * 22f64.to_radians().cos() < 0.02);
    assert!((cal.dec - 22.0).abs() < 0.02);
    assert!((cal.pixscale - 4.0).abs() < 0.05);
    assert!((cal.orientation - truth.orientation()).abs() < 0.5);
    assert_eq!(cal.parity, truth.parity());

    // Second solve nearby must take the warm path and agree.
    let sol2 = engine
        .solve_field(&stars, w, h, &SolveHints::default())
        .expect("warm re-solve");
    assert!((sol2.calibration.ra - cal.ra).abs() < 0.01);
}
