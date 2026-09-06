//! Coarse-grid median background estimation (dmedsmooth equivalent).

use crate::GrayImage;

/// Estimate a smooth background: median over grid cells of ~`cell` pixels,
/// bilinearly interpolated back to full resolution.
pub fn estimate_background(img: &GrayImage, cell: usize) -> Vec<f32> {
    let (w, h) = (img.w, img.h);
    let gx = w.div_ceil(cell).max(1);
    let gy = h.div_ceil(cell).max(1);
    let mut grid = vec![0f32; gx * gy];
    let mut buf: Vec<f32> = Vec::with_capacity(cell * cell);

    for cy in 0..gy {
        for cx in 0..gx {
            let x0 = cx * cell;
            let y0 = cy * cell;
            let x1 = (x0 + cell).min(w);
            let y1 = (y0 + cell).min(h);
            buf.clear();
            // Subsample large cells: medians don't need every pixel.
            let step = (((x1 - x0) * (y1 - y0)) / 4096).max(1);
            let mut i = 0usize;
            for y in y0..y1 {
                for x in x0..x1 {
                    if i.is_multiple_of(step) {
                        buf.push(img.at(x, y));
                    }
                    i += 1;
                }
            }
            let mid = buf.len() / 2;
            grid[cy * gx + cx] = *buf
                .select_nth_unstable_by(mid, |a, b| a.total_cmp(b))
                .1;
        }
    }

    // Bilinear interpolation of grid cell centers back to pixels.
    let mut bg = vec![0f32; w * h];
    let half = cell as f32 / 2.0;
    for y in 0..h {
        let fy = ((y as f32 - half) / cell as f32).max(0.0);
        let gy0 = (fy as usize).min(gy - 1);
        let gy1 = (gy0 + 1).min(gy - 1);
        let ty = (fy - gy0 as f32).clamp(0.0, 1.0);
        for x in 0..w {
            let fx = ((x as f32 - half) / cell as f32).max(0.0);
            let gx0 = (fx as usize).min(gx - 1);
            let gx1 = (gx0 + 1).min(gx - 1);
            let tx = (fx - gx0 as f32).clamp(0.0, 1.0);
            let v00 = grid[gy0 * gx + gx0];
            let v01 = grid[gy0 * gx + gx1];
            let v10 = grid[gy1 * gx + gx0];
            let v11 = grid[gy1 * gx + gx1];
            bg[y * w + x] =
                v00 * (1.0 - tx) * (1.0 - ty) + v01 * tx * (1.0 - ty) + v10 * (1.0 - tx) * ty
                    + v11 * tx * ty;
        }
    }
    bg
}
