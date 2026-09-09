//! `faint-light-tools` — the development and debugging commands, in one
//! binary behind `--features tools`.
//!
//! These are for working on the solver, not for running it: a normal build
//! produces only the server (and the GUI, with `--features gui`). They live
//! in this crate because it is the one that already depends on every layer
//! they poke at.

use std::path::PathBuf;
use std::time::Instant;

use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
faint-light-tools <command> [options]

  solve [options] <image>      plate solve one image against an index directory
    --index-dir DIR            where the index-*.fits files are (default ./indexes)
    --scale-lo A --scale-hi B  pixel scale bounds, arcsec/px
    --ra R --dec D --radius X  position hint, degrees
    --repeat N                 solve N times, to see warm-start timings

  extract <image> [max_stars]  extract stars and print \"x y flux\", brightest first

  index-dump <index.fits>...   index metadata, tree shapes and sanity checks

RUST_LOG controls solver logging, as it does for the server.
";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "solve" => solve(rest),
        "extract" => extract(rest),
        "index-dump" => index_dump(rest),
        "-h" | "--help" | "help" => print!("{USAGE}"),
        "" => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
        other => {
            eprintln!("unknown command {other:?}\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("faint-light-tools: {msg}");
    std::process::exit(2);
}

fn solve(args: Vec<String>) {
    use fl_solve::{Engine, EngineConfig, SolveHints};

    let mut index_dir = PathBuf::from("./indexes");
    let mut hints = SolveHints::default();
    let mut image: Option<PathBuf> = None;
    let mut repeat = 1usize;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut value = |what: &str| {
            it.next()
                .unwrap_or_else(|| die(&format!("{what} needs a value")))
        };
        match a.as_str() {
            "--index-dir" => index_dir = value("--index-dir").into(),
            "--scale-lo" => hints.scale_lo = value("--scale-lo").parse().ok(),
            "--scale-hi" => hints.scale_hi = value("--scale-hi").parse().ok(),
            "--ra" => hints.center_ra = value("--ra").parse().ok(),
            "--dec" => hints.center_dec = value("--dec").parse().ok(),
            "--radius" => hints.radius = value("--radius").parse().ok(),
            "--repeat" => repeat = value("--repeat").parse().unwrap_or(1),
            other => image = Some(other.into()),
        }
    }
    let Some(image) = image else {
        die("solve needs an image")
    };
    let bytes = std::fs::read(&image).unwrap_or_else(|e| die(&format!("{}: {e}", image.display())));

    let t0 = Instant::now();
    let engine = Engine::new(EngineConfig {
        index_dir,
        ..Default::default()
    })
    .unwrap_or_else(|e| die(&format!("engine: {e}")));
    eprintln!(
        "engine: {} indexes loaded in {:?}",
        engine.index_count(),
        t0.elapsed()
    );

    for i in 0..repeat {
        let t = Instant::now();
        match engine.solve_image(&bytes, &hints) {
            Ok(sol) => {
                let c = &sol.calibration;
                println!(
                    "solve #{i}: {:?}  ra={:.5} dec={:.5} pixscale={:.4} orientation={:.3} \
                     parity={} radius={:.3} logodds={:.1} nmatch={} index={}",
                    t.elapsed(),
                    c.ra,
                    c.dec,
                    c.pixscale,
                    c.orientation,
                    c.parity,
                    c.radius,
                    sol.logodds,
                    sol.nmatch,
                    sol.index_id
                );
            }
            Err(e) => {
                println!("solve #{i}: FAILED after {:?}: {e}", t.elapsed());
                std::process::exit(1);
            }
        }
    }
}

fn extract(args: Vec<String>) {
    use fl_extract::{decode, extract_stars, ExtractParams};

    let Some(path) = args.first() else {
        die("extract needs an image")
    };
    let bytes = std::fs::read(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    let img = decode(&bytes).unwrap_or_else(|e| die(&format!("decode: {e}")));
    let mut params = ExtractParams::default();
    if let Some(n) = args.get(1) {
        params.max_stars = n
            .parse()
            .unwrap_or_else(|_| die("max_stars must be a number"));
    }
    let t0 = Instant::now();
    let stars = extract_stars(&img, &params);
    eprintln!(
        "{}x{}, {} stars in {:?}",
        img.w,
        img.h,
        stars.len(),
        t0.elapsed()
    );
    for s in stars {
        println!("{:.3} {:.3} {:.1}", s.x, s.y, s.flux);
    }
}

fn index_dump(args: Vec<String>) {
    use fl_index::IndexFile;

    if args.is_empty() {
        die("index-dump needs at least one index file");
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

fn dump(idx: &fl_index::IndexFile) {
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
        for &v in c.iter().take(codes.ndim()) {
            cmin = cmin.min(v);
            cmax = cmax.max(v);
        }
        if m.cx_le_dx && codes.ndim() >= 4 && c[0] > c[2] + 1e-9 {
            cxdx_viol += 1;
        }
    }
    println!(
        "  sanity: code range [{cmin:.4}, {cmax:.4}], cx<=dx violations: {cxdx_viol} (sampled)"
    );

    // A couple of quads resolve to valid star ids.
    let (q0, dq) = idx.quad_stars(0);
    let ok = q0[..dq].iter().all(|&s| (s as usize) < m.nstars);
    println!("  sanity: quad0 stars {:?} valid={}", &q0[..dq], ok);
}
