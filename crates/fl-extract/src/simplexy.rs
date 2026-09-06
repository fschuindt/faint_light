//! Star detection: threshold + connected components + sub-pixel centroid.

use crate::background::estimate_background;
use crate::GrayImage;

#[derive(Debug, Clone, Copy)]
pub struct Star {
    /// Pixel position, 0-based pixel-center convention, in the ORIGINAL
    /// (pre-downsample) image frame.
    pub x: f64,
    pub y: f64,
    pub flux: f32,
}

#[derive(Debug, Clone)]
pub struct ExtractParams {
    /// Gaussian PSF sigma used for smoothing before detection (pixels).
    pub dpsf: f32,
    /// Detection threshold in sigmas above background.
    pub plim: f32,
    /// Keep at most this many stars (brightest first).
    pub max_stars: usize,
    /// Box-average factor applied before detection (1 = none).
    pub downsample: usize,
    /// Background grid cell in pixels.
    pub bg_cell: usize,
    /// Reject connected components larger than this many pixels
    /// (satellite trails, nebulosity blobs).
    pub max_obj_pixels: usize,
}

impl Default for ExtractParams {
    fn default() -> Self {
        ExtractParams {
            dpsf: 1.0,
            plim: 8.0,
            max_stars: 400,
            downsample: 2,
            bg_cell: 64,
            max_obj_pixels: 5000,
        }
    }
}

/// Estimate pixel noise sigma from differences of pixels 5 apart
/// (astrometry.net dsigma): sigma = quantile_0.68(|diff|) / sqrt(2).
fn estimate_sigma(img: &GrayImage) -> f32 {
    const SP: usize = 5;
    let mut diffs: Vec<f32> = Vec::with_capacity(65536);
    let step = ((img.w * img.h) / 65536).max(1);
    let mut i = 0usize;
    for y in 0..img.h {
        for x in 0..img.w.saturating_sub(SP) {
            if i.is_multiple_of(step) {
                diffs.push((img.at(x, y) - img.at(x + SP, y)).abs());
            }
            i += 1;
        }
    }
    if diffs.is_empty() {
        return 1.0;
    }
    let k = ((diffs.len() as f32) * 0.68) as usize;
    let k = k.min(diffs.len() - 1);
    let q = *diffs.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1;
    (q / std::f32::consts::SQRT_2).max(f32::EPSILON)
}

/// Separable Gaussian smoothing.
fn gaussian_smooth(img: &GrayImage, sigma: f32) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as i64;
    let mut kernel = Vec::with_capacity((2 * radius + 1) as usize);
    let mut sum = 0f32;
    for i in -radius..=radius {
        let v = (-((i * i) as f32) / (2.0 * sigma * sigma)).exp();
        kernel.push(v);
        sum += v;
    }
    for k in &mut kernel {
        *k /= sum;
    }
    let (w, h) = (img.w, img.h);
    let mut tmp = vec![0f32; w * h];
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for (ki, kv) in kernel.iter().enumerate() {
                let xx = (x as i64 + ki as i64 - radius).clamp(0, w as i64 - 1) as usize;
                s += img.data[y * w + xx] * kv;
            }
            tmp[y * w + x] = s;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut s = 0f32;
            for (ki, kv) in kernel.iter().enumerate() {
                let yy = (y as i64 + ki as i64 - radius).clamp(0, h as i64 - 1) as usize;
                s += tmp[yy * w + x] * kv;
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// 3x3 quadratic sub-pixel centroid around a peak (dcen3x3 equivalent).
/// Returns offsets in [-1, 1] relative to the peak pixel.
fn centroid_3x3(v: &[f32; 9]) -> (f64, f64) {
    let axis = |m1: f32, c: f32, p1: f32| -> f64 {
        let denom = m1 - 2.0 * c + p1;
        if denom >= 0.0 {
            // not a maximum along this axis
            return 0.0;
        }
        (0.5 * (m1 - p1) as f64 / denom as f64).clamp(-1.0, 1.0)
    };
    // Sum rows/columns for stability.
    let cx = axis(
        v[0] + v[3] + v[6],
        v[1] + v[4] + v[7],
        v[2] + v[5] + v[8],
    );
    let cy = axis(
        v[0] + v[1] + v[2],
        v[3] + v[4] + v[5],
        v[6] + v[7] + v[8],
    );
    (cx, cy)
}

pub fn extract_stars(orig: &GrayImage, p: &ExtractParams) -> Vec<Star> {
    let ds = p.downsample.max(1);
    let img = orig.downsample(ds);
    let (w, h) = (img.w, img.h);
    if w < 16 || h < 16 {
        return Vec::new();
    }

    let bg = estimate_background(&img, p.bg_cell);
    let mut sub = GrayImage {
        w,
        h,
        data: img
            .data
            .iter()
            .zip(&bg)
            .map(|(v, b)| v - b)
            .collect(),
    };
    let sigma = estimate_sigma(&sub);
    sub.data = gaussian_smooth(&sub, p.dpsf);
    // Smoothing by a Gaussian with sigma s reduces white noise by
    // 1/(2*sqrt(pi)*s); use the effective sigma for thresholding.
    let smooth_sigma = sigma / (2.0 * std::f32::consts::PI.sqrt() * p.dpsf);
    let limit = p.plim * smooth_sigma;

    // Connected components (4-connectivity) over threshold, union-find.
    let mut label = vec![u32::MAX; w * h];
    let mut parent: Vec<u32> = Vec::new();
    fn find(parent: &mut Vec<u32>, mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for y in 0..h {
        for x in 0..w {
            if sub.data[y * w + x] <= limit {
                continue;
            }
            let left = if x > 0 { label[y * w + x - 1] } else { u32::MAX };
            let up = if y > 0 { label[(y - 1) * w + x] } else { u32::MAX };
            let l = match (left, up) {
                (u32::MAX, u32::MAX) => {
                    let l = parent.len() as u32;
                    parent.push(l);
                    l
                }
                (l, u32::MAX) => find(&mut parent, l),
                (u32::MAX, u) => find(&mut parent, u),
                (l, u) => {
                    let rl = find(&mut parent, l);
                    let ru = find(&mut parent, u);
                    if rl != ru {
                        let (lo, hi) = if rl < ru { (rl, ru) } else { (ru, rl) };
                        parent[hi as usize] = lo;
                        lo
                    } else {
                        rl
                    }
                }
            };
            label[y * w + x] = l;
        }
    }

    // Per-component peak + stats.
    #[derive(Clone, Copy)]
    struct Comp {
        peak: f32,
        px: u32,
        py: u32,
        flux: f64,
        npix: u32,
    }
    let mut comps: Vec<Comp> = vec![
        Comp { peak: f32::MIN, px: 0, py: 0, flux: 0.0, npix: 0 };
        parent.len()
    ];
    for y in 0..h {
        for x in 0..w {
            let l = label[y * w + x];
            if l == u32::MAX {
                continue;
            }
            let r = find(&mut parent, l) as usize;
            let v = sub.data[y * w + x];
            let c = &mut comps[r];
            c.flux += v as f64;
            c.npix += 1;
            if v > c.peak {
                c.peak = v;
                c.px = x as u32;
                c.py = y as u32;
            }
        }
    }

    let mut stars: Vec<Star> = Vec::new();
    for (i, c) in comps.iter().enumerate() {
        if c.npix == 0 || parent[i] != i as u32 {
            continue;
        }
        if c.npix as usize > p.max_obj_pixels {
            continue;
        }
        let (px, py) = (c.px as usize, c.py as usize);
        if px == 0 || py == 0 || px == w - 1 || py == h - 1 {
            continue;
        }
        let mut v = [0f32; 9];
        for dy in 0..3 {
            for dx in 0..3 {
                v[dy * 3 + dx] = sub.data[(py + dy - 1) * w + (px + dx - 1)];
            }
        }
        let (ox, oy) = centroid_3x3(&v);
        // Map back to original pixel frame: pixel centers of the f x f box.
        let f = ds as f64;
        stars.push(Star {
            x: (px as f64 + ox) * f + (f - 1.0) / 2.0,
            y: (py as f64 + oy) * f + (f - 1.0) / 2.0,
            flux: c.flux as f32,
        });
    }
    stars.sort_by(|a, b| b.flux.total_cmp(&a.flux));
    stars.truncate(p.max_stars);
    stars
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render fake gaussian stars, extract, and require sub-0.2px accuracy.
    #[test]
    fn synthetic_field_roundtrip() {
        let (w, h) = (512usize, 384usize);
        let mut data = vec![100.0f32; w * h];
        // Deterministic "noise"
        let mut seed = 42u64;
        for v in &mut data {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            *v += ((seed >> 33) as f32 / u32::MAX as f32 - 0.5) * 4.0;
        }
        let truth: Vec<(f64, f64, f32)> = vec![
            (100.25, 50.75, 8000.0),
            (300.5, 200.1, 6000.0),
            (450.9, 350.3, 5000.0),
            (50.0, 300.0, 4000.0),
            (256.6, 100.4, 3000.0),
        ];
        for &(sx, sy, amp) in &truth {
            let s = 1.6f64;
            for y in (sy as usize).saturating_sub(8)..(sy as usize + 8).min(h) {
                for x in (sx as usize).saturating_sub(8)..(sx as usize + 8).min(w) {
                    let d2 = (x as f64 - sx).powi(2) + (y as f64 - sy).powi(2);
                    data[y * w + x] += amp * (-d2 / (2.0 * s * s)).exp() as f32 / 10.0;
                }
            }
        }
        let img = GrayImage { w, h, data };
        let params = ExtractParams {
            downsample: 1,
            ..Default::default()
        };
        let stars = extract_stars(&img, &params);
        assert!(stars.len() >= truth.len(), "found {}", stars.len());
        for &(sx, sy, _) in &truth {
            let best = stars
                .iter()
                .map(|st| ((st.x - sx).powi(2) + (st.y - sy).powi(2)).sqrt())
                .fold(f64::INFINITY, f64::min);
            assert!(best < 0.35, "star at ({sx},{sy}) off by {best}");
        }
    }
}
