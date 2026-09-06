//! Extract stars from an image and print x,y,flux (brightest first).

use fl_extract::{decode, extract_stars, ExtractParams};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: fl-extract-cli <image> [max_stars]");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("read image");
    let img = decode(&bytes).expect("decode image");
    let mut params = ExtractParams::default();
    if let Some(n) = args.get(1) {
        params.max_stars = n.parse().expect("max_stars");
    }
    let t0 = std::time::Instant::now();
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
