//! Layer colours (main overlay PRD, Colour selection): the palette, complements, what shared
//! content prints as, and the readability checks behind the wizard's warnings. A warning is
//! advice, never a block: generation works with any pair.

/// The palette: four complementary pairs, each colour's complement also a swatch, so a pick from
/// the palette with the complementary checkbox on always lands the other colour on a swatch too.
/// The first pair is the default: red on `Old`, cyan on `New`.
pub const PALETTE: [([u8; 3], [u8; 3]); 4] =
    [([204, 51, 51], [51, 204, 204]), ([204, 51, 204], [51, 204, 51]), ([255, 127, 0], [0, 128, 255]), ([255, 64, 159], [0, 191, 96])];

/// Each colour on white paper: WCAG contrast at least this (PRD: about 1.9, PDF Studio's default
/// pair passes; a tuning value, not a standard).
pub const MIN_CONTRAST: f64 = 1.9;
/// The two colours look different: CIE76 ΔE at least this.
pub const MIN_DELTA_E: f64 = 50.0;
/// Shared content prints darker than either colour: the overlap's contrast on white at least this.
pub const MIN_OVERLAP_CONTRAST: f64 = 7.0;

/// The RGB complement, channel by channel (`255 − c`): PDF Studio's rule, kept by the wizard's
/// complementary checkbox.
pub fn complement(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| 255 - v)
}

/// What opaque content present in both documents prints as under Multiply: `A × B / 255` per
/// channel.
pub fn overlap(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    let [ar, ag, ab] = a;
    let [br, bg, bb] = b;
    // Each product is at most 255 × 255 (+127 fits a u16), so the quotient fits a u8.
    let m = |x: u8, y: u8| ((u16::from(x) * u16::from(y) + 127) / 255) as u8;
    [m(ar, br), m(ag, bg), m(ab, bb)]
}

fn linear(c: u8) -> f64 {
    let s = f64::from(c) / 255.0;
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}

/// WCAG relative luminance.
pub fn relative_luminance(c: [u8; 3]) -> f64 {
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}

/// WCAG contrast ratio of `c` against white paper, 1 to 21. An approximate on-screen
/// readability indicator, not a standard for drawing visibility.
pub fn contrast_on_white(c: [u8; 3]) -> f64 {
    1.05 / (relative_luminance(c) + 0.05)
}

/// CIE L*a*b* (D65) of an sRGB colour.
fn lab(c: [u8; 3]) -> [f64; 3] {
    let (r, g, b) = (linear(c[0]), linear(c[1]), linear(c[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.950_47;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.088_83;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 colour difference ΔE*ab.
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (p, q) = (lab(a), lab(b));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// One readability check a colour pair fails.
#[derive(Clone, Debug, PartialEq)]
pub enum ColourWarning {
    /// A colour is too faint on white paper. `old`: it is the `Old` colour.
    Faint { old: bool, contrast: f64 },
    /// The two colours look too alike.
    TooAlike { delta_e: f64 },
    /// Shared content doesn't print clearly darker than the colours.
    LightOverlap { contrast: f64 },
}

impl ColourWarning {
    pub fn text(&self) -> String {
        match self {
            ColourWarning::Faint { old, contrast } => format!(
                "The {} colour is faint on white paper (contrast {contrast:.2}, below {MIN_CONTRAST}): thin lines in it may be hard to see.",
                if *old { "Old" } else { "New" }
            ),
            ColourWarning::TooAlike { delta_e } => {
                format!("The two colours look alike (ΔE {delta_e:.0}, below {MIN_DELTA_E:.0}): differences may be hard to tell apart.")
            }
            ColourWarning::LightOverlap { contrast } => format!(
                "Content in both documents doesn't print much darker than either colour (contrast {contrast:.1}, below {MIN_OVERLAP_CONTRAST:.0})."
            ),
        }
    }
}

/// The checks `old` and `new` fail, in the PRD's order. Empty: the pair passes every check (all
/// four palette pairs do).
pub fn check(old: [u8; 3], new: [u8; 3]) -> Vec<ColourWarning> {
    let mut out = Vec::new();
    for (is_old, c) in [(true, old), (false, new)] {
        let contrast = contrast_on_white(c);
        if contrast < MIN_CONTRAST {
            out.push(ColourWarning::Faint { old: is_old, contrast });
        }
    }
    let d = delta_e(old, new);
    if d < MIN_DELTA_E {
        out.push(ColourWarning::TooAlike { delta_e: d });
    }
    let contrast = contrast_on_white(overlap(old, new));
    if contrast < MIN_OVERLAP_CONTRAST {
        out.push(ColourWarning::LightOverlap { contrast });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_is_closed_under_complements_and_passes() {
        for (a, b) in PALETTE {
            assert_eq!(complement(a), b);
            assert_eq!(complement(b), a);
            assert!(check(a, b).is_empty(), "{a:?} {b:?}: {:?}", check(a, b));
        }
        assert_eq!(overlap([204, 51, 51], [51, 204, 204]), [41, 41, 41]);
        assert!((contrast_on_white([51, 204, 204]) - 1.97).abs() < 0.01);
        assert!((contrast_on_white([204, 51, 51]) - 5.14).abs() < 0.01);
    }

    #[test]
    fn faint_alike_and_light_pairs_are_warned() {
        // Black with white: white is invisible.
        assert!(matches!(check([0, 0, 0], [255, 255, 255]).as_slice(), [ColourWarning::Faint { old: false, .. }]));
        // Gray with its complement: identical-looking.
        assert!(check([128, 128, 128], [127, 127, 127]).iter().any(|w| matches!(w, ColourWarning::TooAlike { .. })));
        // Red with orange: the overlap stays light.
        assert!(check([255, 0, 0], [255, 165, 0]).iter().any(|w| matches!(w, ColourWarning::LightOverlap { .. })));
    }
}
