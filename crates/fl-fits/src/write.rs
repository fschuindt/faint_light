//! Minimal FITS writer: the counterpart to the reader in this crate.
//!
//! It covers exactly what the server emits — a header-only HDU carrying a
//! WCS (what `solve-field` writes as a `.wcs` file), a float image HDU, and
//! injection of a fresh WCS into a FITS the client uploaded, leaving that
//! file's pixels and its unrelated cards untouched.

use crate::{Fits, FitsError, Result, BLOCK, CARD};

/// A header card to serialize.
///
/// `value` is the value field already justified into columns 11-30 (numbers
/// right, strings left, as the fixed format requires); `None` marks a
/// commentary card, whose text occupies columns 9-80 instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub key: String,
    pub value: Option<String>,
    pub comment: String,
}

impl Card {
    /// A card with a pre-justified value field.
    fn new(key: &str, value: String, comment: &str) -> Card {
        Card {
            key: key.to_string(),
            value: Some(value),
            comment: comment.to_string(),
        }
    }

    pub fn int(key: &str, v: i64, comment: &str) -> Card {
        Card::new(key, format!("{v:>20}"), comment)
    }

    pub fn bool(key: &str, v: bool, comment: &str) -> Card {
        Card::new(key, format!("{:>20}", if v { "T" } else { "F" }), comment)
    }

    /// Exponential form with 12 decimals — the precision astrometry.net
    /// writes for WCS terms, and enough to round-trip an f64 pixel scale.
    pub fn float(key: &str, v: f64, comment: &str) -> Card {
        Card::new(key, format!("{:>20}", format!("{v:.12E}")), comment)
    }

    /// Plain fixed-point form, for values whose magnitude is known and
    /// small (`EQUINOX`, `IMAGEW`).
    pub fn decimal(key: &str, v: f64, decimals: usize, comment: &str) -> Card {
        Card::new(key, format!("{:>20}", format!("{v:.decimals$}")), comment)
    }

    /// Single-quoted string, padded to the 8-character minimum the standard
    /// mandates inside the quotes.
    pub fn string(key: &str, v: &str, comment: &str) -> Card {
        let escaped = v.replace('\'', "''");
        Card::new(key, format!("{:<20}", format!("'{escaped:<8}'")), comment)
    }

    pub fn comment(text: &str) -> Card {
        Card {
            key: "COMMENT".into(),
            value: None,
            comment: text.to_string(),
        }
    }

    /// The 80-byte fixed-length card. FITS headers are ASCII: anything else
    /// (and any control character) becomes a space rather than an invalid
    /// file.
    pub fn to_bytes(&self) -> [u8; CARD] {
        let text = match (&self.value, self.comment.is_empty()) {
            (Some(v), true) => format!("{:<8}= {v}", self.key),
            (Some(v), false) => format!("{:<8}= {v} / {}", self.key, self.comment),
            (None, _) => format!("{:<8} {}", self.key, self.comment),
        };
        let mut out = [b' '; CARD];
        for (slot, b) in out.iter_mut().zip(text.bytes()) {
            *slot = if (0x20..0x7f).contains(&b) { b } else { b' ' };
        }
        out
    }
}

/// Serialize `cards`, append END, and pad to a whole number of 2880-byte
/// blocks.
fn header_block(cards: &[Card]) -> Vec<u8> {
    let mut buf = Vec::with_capacity((cards.len() + 1) * CARD);
    for c in cards {
        buf.extend_from_slice(&c.to_bytes());
    }
    let mut end = [b' '; CARD];
    end[..3].copy_from_slice(b"END");
    buf.extend_from_slice(&end);
    buf.resize(buf.len().div_ceil(BLOCK) * BLOCK, b' ');
    buf
}

/// A primary HDU with no data, carrying `cards`. This is the shape of a
/// `solve-field` `.wcs` file.
pub fn header_only(cards: &[Card]) -> Vec<u8> {
    let mut all = vec![
        Card::bool("SIMPLE", true, "conforms to the FITS standard"),
        Card::int("BITPIX", 8, "no data, minimum width"),
        Card::int("NAXIS", 0, "no data"),
        Card::bool("EXTEND", true, ""),
    ];
    all.extend_from_slice(cards);
    header_block(&all)
}

/// A primary HDU holding `data` as 32-bit IEEE floats.
///
/// Rows go out in the order given, first row first — the same array order
/// the star extractor and therefore the WCS use, so WCS cards taken from a
/// solution describe this file directly. (For an image that arrived as
/// JPEG/PNG that means row 1 is the top row, the transpose of the usual
/// bottom-up FITS display convention; the WCS stays correct either way,
/// which is what a client reading the file back cares about.)
pub fn image_f32(width: usize, height: usize, data: &[f32], cards: &[Card]) -> Vec<u8> {
    let npix = width * height;
    let mut all = vec![
        Card::bool("SIMPLE", true, "conforms to the FITS standard"),
        Card::int("BITPIX", -32, "32-bit IEEE floating point"),
        Card::int("NAXIS", 2, ""),
        Card::int("NAXIS1", width as i64, "image width, pixels"),
        Card::int("NAXIS2", height as i64, "image height, pixels"),
    ];
    all.extend_from_slice(cards);
    let mut buf = header_block(&all);
    buf.reserve(npix * 4 + BLOCK);
    for &v in data.iter().take(npix) {
        buf.extend_from_slice(&v.to_be_bytes());
    }
    // A short `data` slice would leave a truncated file; pad with zeros.
    buf.resize(buf.len() + 4 * npix.saturating_sub(data.len()), 0);
    let pad = buf.len().div_ceil(BLOCK) * BLOCK - buf.len();
    buf.resize(buf.len() + pad, 0);
    buf
}

/// The 8-byte keyword of a raw card.
fn keyword(card: &[u8]) -> &str {
    std::str::from_utf8(&card[..8]).unwrap_or("").trim_end()
}

/// Copy `original`, rewriting the header of its first image HDU (the primary
/// HDU if the file has no image) to carry `cards`.
///
/// Existing cards keep their order and content except where `cards`
/// redefines them or `drop` selects them. Dropping matters: a leftover
/// CDELT/CROTA pair or a SIP distortion polynomial would be read alongside
/// the new CD matrix and quietly move the solution.
pub fn set_cards(original: &[u8], cards: &[Card], drop: impl Fn(&str) -> bool) -> Result<Vec<u8>> {
    let fits = Fits::parse(original)?;
    let hdu = fits
        .hdus
        .iter()
        .find(|h| h.header.get_i64("NAXIS").unwrap_or(0) >= 2)
        .or_else(|| fits.hdus.first())
        .ok_or(FitsError::NotFits)?;
    let start = hdu.header_offset();

    let mut raw: Vec<[u8; CARD]> = Vec::new();
    for chunk in original[start..hdu.data_offset].as_chunks::<CARD>().0 {
        let key = keyword(chunk);
        if key == "END" && chunk[8..].iter().all(|&b| b == b' ') {
            break;
        }
        if chunk.iter().all(|&b| b == b' ') {
            continue; // padding inside a multi-block header
        }
        // Only value-bearing cards displace by keyword: COMMENT and HISTORY
        // are free text, so matching on those would erase the file's own
        // provenance instead of replacing a keyword.
        if cards.iter().any(|c| c.value.is_some() && c.key == key) || drop(key) {
            continue;
        }
        raw.push(*chunk);
    }
    // Rebuild the header from the retained raw cards plus the new ones. Raw
    // cards are re-emitted verbatim, so nothing the reader cannot represent
    // (CONTINUE, HIERARCH, odd spacing) is lost.
    let mut buf = Vec::with_capacity(original.len() + BLOCK);
    buf.extend_from_slice(&original[..start]);
    let mut header = Vec::with_capacity((raw.len() + cards.len() + 1) * CARD);
    for c in &raw {
        header.extend_from_slice(c);
    }
    for c in cards {
        header.extend_from_slice(&c.to_bytes());
    }
    let mut end = [b' '; CARD];
    end[..3].copy_from_slice(b"END");
    header.extend_from_slice(&end);
    header.resize(header.len().div_ceil(BLOCK) * BLOCK, b' ');
    buf.extend_from_slice(&header);
    buf.extend_from_slice(&original[hdu.data_offset..]);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_text(c: &Card) -> String {
        String::from_utf8(c.to_bytes().to_vec()).unwrap()
    }

    #[test]
    fn card_layout_matches_fixed_format() {
        // Value field ends at column 30, comment follows " / ".
        let c = Card::int("BITPIX", -32, "32-bit float");
        assert_eq!(
            card_text(&c),
            format!("{:<80}", "BITPIX  =                  -32 / 32-bit float")
        );
        let s = Card::string("CTYPE1", "RA---TAN", "TAN projection");
        assert_eq!(
            card_text(&s),
            format!("{:<80}", "CTYPE1  = 'RA---TAN'           / TAN projection")
        );
        // Short strings are padded to the eight-character minimum.
        assert!(card_text(&Card::string("CUNIT1", "deg", "")).starts_with("CUNIT1  = 'deg     '"));
    }

    #[test]
    fn non_ascii_is_sanitized() {
        let c = Card::string("OBJECT", "M42\u{2014}core", "caf\u{e9}");
        let text = card_text(&c);
        assert!(text.is_ascii());
        assert_eq!(text.len(), CARD);
    }

    #[test]
    fn image_round_trips_through_the_reader() {
        let data: Vec<f32> = (0..12).map(|i| i as f32 * 0.5).collect();
        let buf = image_f32(4, 3, &data, &[Card::string("OBJECT", "test", "")]);
        assert_eq!(buf.len() % BLOCK, 0);
        let img = crate::image::decode_image(&buf).unwrap();
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.data, data);
        let fits = Fits::parse(&buf).unwrap();
        assert_eq!(fits.hdus[0].header.get_str("OBJECT"), Some("test"));
    }

    #[test]
    fn header_only_has_no_data() {
        let buf = header_only(&[Card::float("CRVAL1", 83.94, "RA")]);
        assert_eq!(buf.len(), BLOCK);
        let fits = Fits::parse(&buf).unwrap();
        assert_eq!(fits.hdus.len(), 1);
        assert_eq!(fits.hdus[0].data_len, 0);
        assert!((fits.hdus[0].header.get_f64("CRVAL1").unwrap() - 83.94).abs() < 1e-9);
    }

    #[test]
    fn set_cards_replaces_wcs_and_keeps_pixels() {
        let data: Vec<f32> = (0..6).map(|i| i as f32).collect();
        let original = image_f32(
            3,
            2,
            &data,
            &[
                Card::string("OBJECT", "M42", "target"),
                Card::comment("acquired by the observatory"),
                Card::float("CRVAL1", 10.0, "old"),
                Card::float("CDELT1", 0.001, "stale convention"),
            ],
        );
        let out = set_cards(
            &original,
            &[
                Card::float("CRVAL1", 83.94, "new"),
                Card::comment("solved by faint_light"),
            ],
            |k| k == "CDELT1",
        )
        .unwrap();
        let fits = Fits::parse(&out).unwrap();
        let h = &fits.hdus[0].header;
        assert_eq!(h.get_str("OBJECT"), Some("M42"));
        // Commentary cards accumulate; they never displace each other.
        let text = String::from_utf8_lossy(&out[..BLOCK]);
        assert!(text.contains("acquired by the observatory"));
        assert!(text.contains("solved by faint_light"));
        assert!((h.get_f64("CRVAL1").unwrap() - 83.94).abs() < 1e-9);
        assert_eq!(h.get_f64("CDELT1"), None);
        assert_eq!(crate::image::decode_image(&out).unwrap().data, data);
    }

    #[test]
    fn set_cards_grows_the_header_across_a_block_boundary() {
        let original = image_f32(2, 2, &[1.0, 2.0, 3.0, 4.0], &[]);
        assert_eq!(original.len(), 2 * BLOCK);
        let many: Vec<Card> = (0..40)
            .map(|i| Card::int(&format!("KEY{i:05}"), i, "filler"))
            .collect();
        let out = set_cards(&original, &many, |_| false).unwrap();
        assert_eq!(out.len(), 3 * BLOCK);
        let fits = Fits::parse(&out).unwrap();
        assert_eq!(fits.hdus[0].header.nblocks, 2);
        assert_eq!(fits.hdus[0].header.get_i64("KEY00039"), Some(39));
        assert_eq!(
            crate::image::decode_image(&out).unwrap().data,
            vec![1.0, 2.0, 3.0, 4.0]
        );
    }
}
