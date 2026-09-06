//! Decode FITS image HDUs to f32 grayscale.
//!
//! Unlike the astrometry.net index tables, image data follows the FITS
//! standard: big-endian, with BZERO/BSCALE applied (BZERO=32768 is the
//! usual unsigned-16-bit convention used by capture software).

use crate::{Fits, FitsError, Hdu, Result};

pub struct FitsImage {
    pub width: usize,
    pub height: usize,
    /// Row-major luminance, length width*height.
    pub data: Vec<f32>,
}

/// Decode the first image HDU (NAXIS >= 2) found in the file.
/// For NAXIS3 = 3 (RGB planes) the planes are averaged.
pub fn decode_image(buf: &[u8]) -> Result<FitsImage> {
    let fits = Fits::parse(buf)?;
    for hdu in &fits.hdus {
        let naxis = hdu.header.get_i64("NAXIS").unwrap_or(0);
        if naxis >= 2 {
            return decode_hdu(buf, hdu);
        }
    }
    Err(FitsError::Unsupported("no image HDU found".into()))
}

fn decode_hdu(buf: &[u8], hdu: &Hdu) -> Result<FitsImage> {
    let bitpix = hdu.header.req_i64("BITPIX")?;
    let w = hdu.header.req_i64("NAXIS1")?.max(0) as usize;
    let h = hdu.header.req_i64("NAXIS2")?.max(0) as usize;
    let planes = hdu.header.get_i64("NAXIS3").unwrap_or(1).max(1) as usize;
    let bzero = hdu.header.get_f64("BZERO").unwrap_or(0.0);
    let bscale = hdu.header.get_f64("BSCALE").unwrap_or(1.0);
    let npix = w * h;
    let bpp = (bitpix.unsigned_abs() as usize) / 8;
    let need = npix * planes * bpp;
    let data = buf
        .get(hdu.data_offset..hdu.data_offset + need)
        .ok_or(FitsError::Truncated(hdu.data_offset))?;

    let mut out = vec![0f32; npix];
    let scale_plane = 1.0 / planes as f32;
    for p in 0..planes {
        let plane = &data[p * npix * bpp..(p + 1) * npix * bpp];
        for i in 0..npix {
            let raw = match bitpix {
                8 => plane[i] as f64,
                16 => i16::from_be_bytes([plane[2 * i], plane[2 * i + 1]]) as f64,
                32 => i32::from_be_bytes(plane[4 * i..4 * i + 4].try_into().unwrap()) as f64,
                -32 => f32::from_be_bytes(plane[4 * i..4 * i + 4].try_into().unwrap()) as f64,
                -64 => f64::from_be_bytes(plane[8 * i..8 * i + 8].try_into().unwrap()),
                _ => {
                    return Err(FitsError::Unsupported(format!("BITPIX {bitpix}")));
                }
            };
            out[i] += ((bzero + bscale * raw) as f32) * scale_plane;
        }
    }
    Ok(FitsImage {
        width: w,
        height: h,
        data: out,
    })
}
