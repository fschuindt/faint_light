//! Solve an image from the command line.
//!
//! usage: fl-solve-cli --index-dir DIR [--scale-lo A --scale-hi B] [--repeat N] <image>

use std::path::PathBuf;
use std::time::Instant;

use fl_solve::{Engine, EngineConfig, SolveHints};

fn main() {
    tracing_shim::init();
    let mut index_dir = PathBuf::from("./indexes");
    let mut hints = SolveHints::default();
    let mut image: Option<PathBuf> = None;
    let mut repeat = 1usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--index-dir" => index_dir = args.next().expect("--index-dir value").into(),
            "--scale-lo" => hints.scale_lo = Some(args.next().unwrap().parse().unwrap()),
            "--scale-hi" => hints.scale_hi = Some(args.next().unwrap().parse().unwrap()),
            "--ra" => hints.center_ra = Some(args.next().unwrap().parse().unwrap()),
            "--dec" => hints.center_dec = Some(args.next().unwrap().parse().unwrap()),
            "--radius" => hints.radius = Some(args.next().unwrap().parse().unwrap()),
            "--repeat" => repeat = args.next().unwrap().parse().unwrap(),
            other => image = Some(other.into()),
        }
    }
    let image = image.expect("usage: fl-solve-cli --index-dir DIR <image>");
    let bytes = std::fs::read(&image).expect("read image");

    let t0 = Instant::now();
    let engine = Engine::new(EngineConfig {
        index_dir,
        ..Default::default()
    })
    .expect("engine init");
    eprintln!("engine: {} indexes loaded in {:?}", engine.index_count(), t0.elapsed());

    for i in 0..repeat {
        let t = Instant::now();
        match engine.solve_image(&bytes, &hints) {
            Ok(sol) => {
                let c = &sol.calibration;
                println!(
                    "solve #{i}: {:?}  ra={:.5} dec={:.5} pixscale={:.4} orientation={:.3} parity={} radius={:.3} logodds={:.1} nmatch={} index={}",
                    t.elapsed(), c.ra, c.dec, c.pixscale, c.orientation, c.parity, c.radius,
                    sol.logodds, sol.nmatch, sol.index_id
                );
            }
            Err(e) => {
                println!("solve #{i}: FAILED after {:?}: {e}", t.elapsed());
                std::process::exit(1);
            }
        }
    }
}

mod tracing_shim {
    pub fn init() {
        // fl-solve uses tracing; a tiny subscriber keeps the CLI dependency-free.
        struct Stderr;
        use tracing::field::{Field, Visit};
        use tracing::{Event, Subscriber};
        use tracing_core::span;
        impl Subscriber for Stderr {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                std::env::var("FL_LOG").is_ok()
            }
            fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
                span::Id::from_u64(1)
            }
            fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
            fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
            fn event(&self, event: &Event<'_>) {
                struct V(String);
                impl Visit for V {
                    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                        use std::fmt::Write;
                        let _ = write!(self.0, " {}={v:?}", f.name());
                    }
                }
                let mut v = V(String::new());
                event.record(&mut v);
                eprintln!("[{}]{}", event.metadata().target(), v.0);
            }
            fn enter(&self, _: &span::Id) {}
            fn exit(&self, _: &span::Id) {}
        }
        let _ = tracing::subscriber::set_global_default(Stderr);
    }
}
