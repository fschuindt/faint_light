//! Inspect an astrometry.net index file: metadata, tree shapes, sanity checks.

use std::path::PathBuf;

use fl_index::IndexFile;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: fl-index-dump <index.fits>...");
        std::process::exit(2);
    }
    for arg in args {
        let path = PathBuf::from(&arg);
        match IndexFile::open(&path) {
            Ok(idx) => dump(&idx),
            Err(e) => {
                eprintln!("{arg}: ERROR {e}");
                std::process::exit(1);
            }
        }
    }
}

fn dump(idx: &IndexFile) {
    let m = &idx.meta;
    println!("== {}", idx.path.display());
    println!(
        "  id={} healpix={} nside={} allsky={} dimquads={}",
        m.index_id, m.healpix, m.hpnside, m.allsky, m.dimquads
    );
    println!(
        "  quad scale [{:.2}, {:.2}] arcmin  nquads={} nstars={}",
        m.scale_lower_rad.to_degrees() * 60.0,
        m.scale_upper_rad.to_degrees() * 60.0,
        m.nquads,
        m.nstars
    );
    println!(
        "  codes: ndim={} ndata={} nnodes={} nbottom={}",
        idx.codes().tree.ndim,
        idx.codes().tree.ndata,
        idx.codes().tree.nnodes,
        idx.codes().tree.nbottom
    );
    println!(
        "  stars: ndim={} ndata={} nnodes={} nbottom={}",
        idx.stars().tree.ndim,
        idx.stars().tree.ndata,
        idx.stars().tree.nnodes,
        idx.stars().tree.nbottom
    );
    println!(
        "  invariants: cx<=dx={} meanx<=1/2={} circle={}",
        m.cx_le_dx, m.meanx_le_half, m.circle
    );

    // Sanity: star vectors on the unit sphere, codes within bounds.
    let stars = idx.stars();
    let mut worst = 0.0f64;
    let mut p = [0f64; 3];
    for i in (0..stars.ndata()).step_by((stars.ndata() / 1000).max(1)) {
        stars.point(i, &mut p);
        let norm = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        worst = worst.max((norm - 1.0).abs());
    }
    println!("  sanity: max |star|-1 = {worst:.2e} (sampled)");

    let codes = idx.codes();
    let mut cmin = f64::INFINITY;
    let mut cmax = f64::NEG_INFINITY;
    let mut c = [0f64; 8];
    let mut cxdx_viol = 0usize;
    for i in (0..codes.ndata()).step_by((codes.ndata() / 1000).max(1)) {
        codes.point(i, &mut c[..codes.ndim()]);
        for d in 0..codes.ndim() {
            cmin = cmin.min(c[d]);
            cmax = cmax.max(c[d]);
        }
        if m.cx_le_dx && codes.ndim() >= 4 && c[0] > c[2] + 1e-9 {
            cxdx_viol += 1;
        }
    }
    println!("  sanity: code range [{cmin:.4}, {cmax:.4}], cx<=dx violations: {cxdx_viol} (sampled)");

    // A couple of quads resolve to valid star ids.
    let (q0, dq) = idx.quad_stars(0);
    let ok = q0[..dq].iter().all(|&s| (s as usize) < m.nstars);
    println!("  sanity: quad0 stars {:?} valid={}", &q0[..dq], ok);
}
