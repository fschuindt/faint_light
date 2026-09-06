use crate::{FitsError, Result, BLOCK, CARD};

/// A parsed header card value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Value section present but unparseable; kept verbatim.
    Raw(String),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// A FITS header: ordered cards with typed access by keyword.
#[derive(Debug, Clone, Default)]
pub struct Header {
    cards: Vec<(String, Value)>,
    /// Number of 2880-byte blocks this header occupied on disk.
    pub nblocks: usize,
}

impl Header {
    /// Parse a header starting at `off` in `buf`. Returns the header and the
    /// byte offset of the data section that follows it.
    pub fn parse(buf: &[u8], mut off: usize) -> Result<(Header, usize)> {
        let mut cards = Vec::new();
        let start = off;
        loop {
            let block = buf
                .get(off..off + BLOCK)
                .ok_or(FitsError::Truncated(off))?;
            let mut end_found = false;
            for i in 0..BLOCK / CARD {
                let card = &block[i * CARD..(i + 1) * CARD];
                let key = std::str::from_utf8(&card[..8])
                    .map_err(|_| FitsError::BadCard(format!("{:?}", &card[..8])))?
                    .trim_end()
                    .to_string();
                // END card: keyword exactly "END", rest blank. Careful:
                // "ENDIAN" also starts with "END" but has more in the keyword.
                if key == "END" && card[8..].iter().all(|&b| b == b' ') {
                    end_found = true;
                    break;
                }
                if key.is_empty() || key == "COMMENT" || key == "HISTORY" {
                    continue;
                }
                // Value indicator "= " at bytes 8..10.
                if card.len() > 9 && card[8] == b'=' {
                    let raw = std::str::from_utf8(&card[9..])
                        .map_err(|_| FitsError::BadCard(key.clone()))?;
                    let val = parse_value(raw);
                    cards.push((key, val));
                }
                // else: commentary card with unknown keyword — skip.
            }
            off += BLOCK;
            if end_found {
                break;
            }
            // Safety valve: a header longer than 10 MB is not a real header.
            if off - start > 10 << 20 {
                return Err(FitsError::BadCard("unterminated header".into()));
            }
        }
        let nblocks = (off - start) / BLOCK;
        Ok((Header { cards, nblocks }, off))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.cards.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(|v| v.as_i64())
    }
    pub fn get_f64(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|v| v.as_f64())
    }
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|v| v.as_bool())
    }
    pub fn req_i64(&self, key: &str) -> Result<i64> {
        self.get_i64(key)
            .ok_or_else(|| FitsError::MissingCard(key.into()))
    }
    pub fn req_f64(&self, key: &str) -> Result<f64> {
        self.get_f64(key)
            .ok_or_else(|| FitsError::MissingCard(key.into()))
    }
    pub fn cards(&self) -> &[(String, Value)] {
        &self.cards
    }
}

/// Parse the value section of a card (everything after "= ").
fn parse_value(raw: &str) -> Value {
    let raw = raw.trim_start();
    if let Some(rest) = raw.strip_prefix('\'') {
        // String value: ends at a single quote (doubled quotes escape).
        let mut s = String::new();
        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    s.push('\'');
                    chars.next();
                } else {
                    break;
                }
            } else {
                s.push(c);
            }
        }
        return Value::Str(s.trim_end().to_string());
    }
    // Strip trailing comment.
    let val = raw.split('/').next().unwrap_or("").trim();
    match val {
        "T" => Value::Bool(true),
        "F" => Value::Bool(false),
        "" => Value::Raw(String::new()),
        _ => {
            if let Ok(i) = val.parse::<i64>() {
                Value::Int(i)
            } else if let Ok(f) = val.parse::<f64>() {
                Value::Float(f)
            } else {
                Value::Raw(val.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(CARD, b' ');
        v
    }

    fn block(cards: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        for c in cards {
            v.extend_from_slice(&card(c));
        }
        v.resize(BLOCK, b' ');
        v
    }

    #[test]
    fn parses_types_and_endian_vs_end() {
        let buf = block(&[
            "SIMPLE  =                    T / Standard FITS file",
            "BITPIX  =                    8",
            "NAXIS   =                    0",
            "ENDIAN  = '04:03:02:01' / Endianness detector",
            "SCALE_U =      0.0247254977366 / Upper-bound",
            "NQUADS  =               580800 / Number of quads.",
            "END",
        ]);
        let (h, off) = Header::parse(&buf, 0).unwrap();
        assert_eq!(off, BLOCK);
        assert_eq!(h.get_bool("SIMPLE"), Some(true));
        assert_eq!(h.get_i64("BITPIX"), Some(8));
        assert_eq!(h.get_str("ENDIAN"), Some("04:03:02:01"));
        assert!((h.get_f64("SCALE_U").unwrap() - 0.0247254977366).abs() < 1e-15);
        assert_eq!(h.get_i64("NQUADS"), Some(580800));
    }

    #[test]
    fn string_with_escaped_quote() {
        let buf = block(&["TTYPE1  = 'it''s   '           / label", "END"]);
        let (h, _) = Header::parse(&buf, 0).unwrap();
        assert_eq!(h.get_str("TTYPE1"), Some("it's"));
    }

    #[test]
    fn multiblock_header() {
        let mut cards: Vec<String> = vec!["SIMPLE  =                    T".into()];
        for i in 0..40 {
            cards.push(format!("HISTORY entry number {i}"));
        }
        cards.push("NAXIS   =                    0".into());
        cards.push("END".into());
        let refs: Vec<&str> = cards.iter().map(|s| s.as_str()).collect();
        // 42 cards > 36, so two blocks.
        let mut buf = Vec::new();
        for chunk in refs.chunks(36) {
            buf.extend_from_slice(&block(chunk));
        }
        let (h, off) = Header::parse(&buf, 0).unwrap();
        assert_eq!(off, 2 * BLOCK);
        assert_eq!(h.nblocks, 2);
        assert_eq!(h.get_i64("NAXIS"), Some(0));
    }
}
