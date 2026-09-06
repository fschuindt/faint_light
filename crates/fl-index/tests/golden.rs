//! Golden test: kd-tree range search must agree exactly with a brute-force
//! linear scan over the unpacked data points, on real index files.
//!
//! Gated on FAINT_LIGHT_TEST_INDEX_DIR pointing at a directory of
//! index-*.fits files (e.g. the docker_astrometry indexes/ dir).

use fl_index::{IndexFile, KdView};

struct XorShift(u64);
impl XorShift {
    fn next_f64(&mut self) -> f64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545F4914F6CDD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn brute_force(view: &KdView, query: &[f64], r2: f64) -> Vec<(u32, f64)> {
    let mut out = Vec::new();
    let mut p = [0f64; 8];
    for i in 0..view.ndata() {
        view.point(i, &mut p[..view.ndim()]);
        let d2: f64 = (0..view.ndim()).map(|d| (p[d] - query[d]).powi(2)).sum();
        if d2 <= r2 {
            out.push((i as u32, d2));
        }
    }
    out
}

fn check_tree(view: &KdView, nqueries: usize, radius: f64, rng: &mut XorShift, label: &str) {
    let ndim = view.ndim();
    let r2 = radius * radius;
    let mut p = [0f64; 8];
    for qi in 0..nqueries {
        // Half the queries centered on actual data points (guaranteed hits),
        // half uniform in the bounding box.
        let mut query = vec![0f64; ndim];
        if qi % 2 == 0 {
            let i = (rng.next_f64() * view.ndata() as f64) as usize % view.ndata();
            view.point(i, &mut p[..ndim]);
            for d in 0..ndim {
                query[d] = p[d] + (rng.next_f64() - 0.5) * radius;
            }
        } else {
            for d in 0..ndim {
                let (lo, hi) = (view.tree.minval[d], view.tree.maxval[d]);
                query[d] = lo + rng.next_f64() * (hi - lo);
            }
        }
        let mut got = Vec::new();
        view.range_search(&query, r2, &mut got);
        let mut want = brute_force(view, &query, r2);
        got.sort_by_key(|(i, _)| *i);
        want.sort_by_key(|(i, _)| *i);
        assert_eq!(
            got.len(),
            want.len(),
            "{label}: query {qi} count mismatch (got {} want {})",
            got.len(),
            want.len()
        );
        for ((gi, gd), (wi, wd)) in got.iter().zip(&want) {
            assert_eq!(gi, wi, "{label}: query {qi} index mismatch");
            assert!((gd - wd).abs() < 1e-12, "{label}: query {qi} dist mismatch");
        }
    }
}

#[test]
fn range_search_matches_brute_force() {
    let Ok(dir) = std::env::var("FAINT_LIGHT_TEST_INDEX_DIR") else {
        eprintln!("FAINT_LIGHT_TEST_INDEX_DIR not set; skipping golden test");
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("read index dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("index-") && n.ends_with(".fits"))
        })
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no index files in {dir}");

    let mut rng = XorShift(0x5EED_CAFE_F00D_1234);
    for path in paths {
        let idx = IndexFile::open(&path).expect("open index");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        // Codes: codetol-sized and larger radii.
        // Scale query count with tree size to keep runtime sane.
        let nq = (200_000 / (idx.codes().ndata() / 100 + 1)).clamp(20, 300);
        check_tree(&idx.codes(), nq, 0.01, &mut rng, &format!("{name} codes r=0.01"));
        check_tree(&idx.codes(), nq / 2, 0.1, &mut rng, &format!("{name} codes r=0.1"));
        // Stars: ~1 degree and ~5 degree chord radii.
        let r1 = 2.0 * (0.5f64.to_radians()).sin();
        let r5 = 2.0 * (2.5f64.to_radians()).sin();
        check_tree(&idx.stars(), nq, r1, &mut rng, &format!("{name} stars r=1deg"));
        check_tree(&idx.stars(), nq / 2, r5, &mut rng, &format!("{name} stars r=5deg"));
        // Star norms.
        let stars = idx.stars();
        let mut p = [0f64; 3];
        for i in 0..stars.ndata().min(5000) {
            stars.point(i, &mut p);
            let n = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!(
                (n - 1.0).abs() < 1e-3,
                "{name}: star {i} norm {n} far from unit"
            );
        }
        // All quads reference valid stars.
        for q in (0..idx.meta.nquads).step_by((idx.meta.nquads / 2000).max(1)) {
            let (stars_q, dq) = idx.quad_stars(q);
            for &s in &stars_q[..dq] {
                assert!((s as usize) < idx.meta.nstars, "{name}: quad {q} bad star {s}");
            }
        }
        eprintln!("{name}: OK");
    }
}
