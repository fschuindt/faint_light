//! All-sky chart rendering for solved images: constellation stick figures,
//! an alt/az grid, and the solved field-of-view footprint, drawn as SVG on
//! an azimuthal-equidistant projection (zenith at center, horizon at rim).

pub mod altaz;
pub mod chart;

use std::sync::OnceLock;

/// A principal star of a constellation figure: J2000 RA/Dec in degrees
/// (RA in [0, 360)) and visual magnitude.
#[derive(Debug, Clone, Copy)]
pub struct FigureStar {
    pub ra: f64,
    pub dec: f64,
    pub mag: f64,
}

/// One constellation stick figure (IAU abbreviation, display name, label
/// anchor, and the polylines connecting its principal stars).
#[derive(Debug)]
pub struct Constellation {
    pub abbr: &'static str,
    pub name: &'static str,
    pub label: (f64, f64),
    pub polylines: Vec<Vec<FigureStar>>,
}

/// The embedded constellation dataset (88 IAU constellations; Serpens
/// appears twice, as Caput and Cauda).
pub fn constellations() -> &'static [Constellation] {
    static DATA: OnceLock<Vec<Constellation>> = OnceLock::new();
    DATA.get_or_init(|| parse_dataset(include_str!("../data/constellations.txt")))
}

fn parse_dataset(text: &'static str) -> Vec<Constellation> {
    let mut out: Vec<Constellation> = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let bad = || panic!("constellations.txt line {}: malformed: {line}", ln + 1);
        if let Some(rest) = line.strip_prefix("C ") {
            let mut it = rest.split('|');
            let (Some(abbr), Some(name), Some(ra), Some(dec)) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                bad()
            };
            out.push(Constellation {
                abbr,
                name,
                label: (
                    ra.parse().unwrap_or_else(|_| bad()),
                    dec.parse().unwrap_or_else(|_| bad()),
                ),
                polylines: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("P ") {
            let poly: Vec<FigureStar> = rest
                .split_whitespace()
                .map(|triple| {
                    let mut it = triple.split(',');
                    let (Some(ra), Some(dec), Some(mag), None) =
                        (it.next(), it.next(), it.next(), it.next())
                    else {
                        bad()
                    };
                    FigureStar {
                        ra: ra.parse().unwrap_or_else(|_| bad()),
                        dec: dec.parse().unwrap_or_else(|_| bad()),
                        mag: mag.parse().unwrap_or_else(|_| bad()),
                    }
                })
                .collect();
            out.last_mut().unwrap_or_else(|| bad()).polylines.push(poly);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_parses() {
        let all = constellations();
        assert_eq!(all.len(), 89); // 88 constellations, Serpens split in two
        for c in all {
            assert!(!c.polylines.is_empty(), "{} has no lines", c.abbr);
            assert!((0.0..360.0).contains(&c.label.0), "{}", c.abbr);
            assert!((-90.0..=90.0).contains(&c.label.1), "{}", c.abbr);
            for poly in &c.polylines {
                assert!(poly.len() >= 2);
                for s in poly {
                    assert!((0.0..360.0).contains(&s.ra) && (-90.0..=90.0).contains(&s.dec));
                    assert!((-2.0..7.0).contains(&s.mag), "{} mag {}", c.abbr, s.mag);
                }
            }
        }
        let orion = all.iter().find(|c| c.abbr == "Ori").expect("Orion");
        assert_eq!(orion.name, "Orion");
        assert!(orion.polylines.iter().map(|p| p.len() - 1).sum::<usize>() >= 5);
        // Sirius anchors Canis Major — brightest star in the sky.
        let cma = all.iter().find(|c| c.abbr == "CMa").unwrap();
        let sirius = cma
            .polylines
            .iter()
            .flatten()
            .find(|s| (s.ra - 101.287).abs() < 0.01 && (s.dec + 16.716).abs() < 0.01)
            .expect("Sirius in CMa figure");
        assert!(sirius.mag < -1.0, "Sirius mag {}", sirius.mag);
    }
}
