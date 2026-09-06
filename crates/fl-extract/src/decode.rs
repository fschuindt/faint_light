use crate::Result;

/// Grayscale float image, row-major, origin top-left.
pub struct GrayImage {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl GrayImage {
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.w + x]
    }

    /// Box-average downsample by an integer factor.
    pub fn downsample(&self, f: usize) -> GrayImage {
        if f <= 1 {
            return GrayImage {
                w: self.w,
                h: self.h,
                data: self.data.clone(),
            };
        }
        let (nw, nh) = (self.w / f, self.h / f);
        let mut out = vec![0f32; nw * nh];
        let inv = 1.0 / (f * f) as f32;
        for y in 0..nh {
            for x in 0..nw {
                let mut s = 0f32;
                for dy in 0..f {
                    let row = (y * f + dy) * self.w + x * f;
                    for dx in 0..f {
                        s += self.data[row + dx];
                    }
                }
                out[y * nw + x] = s * inv;
            }
        }
        GrayImage { w: nw, h: nh, data: out }
    }
}

/// Decode raw upload bytes into a grayscale image. Sniffs FITS by magic,
/// otherwise defers to the `image` crate (PNG/JPEG/TIFF, 8/16-bit, RGB→luma).
pub fn decode(bytes: &[u8]) -> Result<GrayImage> {
    if bytes.starts_with(b"SIMPLE  =") {
        // Keep FITS rows in array order (row 0 first), exactly like the
        // astrometry.net pipeline does — orientation/parity conventions in
        // the reported calibration depend on this (nova applies a
        // "jpeg-like" 180-degree correction for non-FITS inputs instead).
        let img = fl_fits::image::decode_image(bytes)?;
        return Ok(GrayImage {
            w: img.width,
            h: img.height,
            data: img.data,
        });
    }
    let dyn_img = image::load_from_memory(bytes)?;
    let luma = dyn_img.to_luma32f();
    let (w, h) = (luma.width() as usize, luma.height() as usize);
    Ok(GrayImage {
        w,
        h,
        data: luma.into_raw(),
    })
}
