//! Real-world length units for measuring drawings (this fork, OV10): inches, feet, feet and
//! inches to a fraction of an inch, centimetres, millimetres and metres.
//!
//! Feet and inches are written as a two-element number format array (ISO 32000-2 §12.9.2):
//! whole feet, truncated, then the remainder in inches (`/C 12`) as a fraction with
//! denominator `/D`. PdfCraft draws its own caption, `12'-5 3/8"`, in ASCII: the caption font
//! (Helvetica, WinAnsiEncoding) has no prime marks.

use serde::Serialize;

/// The units the Measure panel offers. Any other label is a custom decimal unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LengthUnit {
    Inches,
    Feet,
    /// Feet and inches to the nearest 1/8 inch.
    FeetInches,
    Centimeters,
    Millimeters,
    Meters,
}

/// The fraction of an inch feet-and-inches values are rounded to.
pub const EIGHTHS: u32 = 8;

impl LengthUnit {
    pub const ALL: [LengthUnit; 6] =
        [LengthUnit::Inches, LengthUnit::Feet, LengthUnit::FeetInches, LengthUnit::Centimeters, LengthUnit::Millimeters, LengthUnit::Meters];

    /// How many inches one of this unit is (feet and inches count in feet).
    pub fn inches(self) -> f64 {
        match self {
            LengthUnit::Inches => 1.0,
            LengthUnit::Feet | LengthUnit::FeetInches => 12.0,
            LengthUnit::Centimeters => 1.0 / 2.54,
            LengthUnit::Millimeters => 1.0 / 25.4,
            LengthUnit::Meters => 1.0 / 0.0254,
        }
    }

    /// The label a scale in this unit carries (`/U`; feet and inches lead with feet).
    pub fn label(self) -> &'static str {
        match self {
            LengthUnit::Inches => "in",
            LengthUnit::Feet | LengthUnit::FeetInches => "ft",
            LengthUnit::Centimeters => "cm",
            LengthUnit::Millimeters => "mm",
            LengthUnit::Meters => "m",
        }
    }

    /// Areas measured in this unit are in its square, written as PdfCraft writes them (`in^2`).
    pub fn area_label(self) -> &'static str {
        match self {
            LengthUnit::Inches => "in^2",
            LengthUnit::Feet | LengthUnit::FeetInches => "ft^2",
            LengthUnit::Centimeters => "cm^2",
            LengthUnit::Millimeters => "mm^2",
            LengthUnit::Meters => "m^2",
        }
    }

    /// The name tools and settings use (`ft-in` for feet and inches).
    pub fn token(self) -> &'static str {
        match self {
            LengthUnit::FeetInches => "ft-in",
            other => other.label(),
        }
    }

    pub fn from_token(s: &str) -> Option<LengthUnit> {
        LengthUnit::ALL.into_iter().find(|u| u.token() == s.trim())
    }

    /// The decimal unit a scale's label names, if it is one of these.
    pub fn from_label(s: &str) -> Option<LengthUnit> {
        [LengthUnit::Inches, LengthUnit::Feet, LengthUnit::Centimeters, LengthUnit::Millimeters, LengthUnit::Meters]
            .into_iter()
            .find(|u| u.label() == s.trim())
    }

    /// `value` of `from` in this unit (feet and inches count in feet).
    pub fn convert(self, value: f64, from: LengthUnit) -> f64 {
        value * from.inches() / self.inches()
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs().max(1) } else { gcd(b, a % b) }
}

/// The largest whole number of `1/denominator` inches that's formatted as feet and inches;
/// beyond it (some 10^14 feet) the value is shown in decimal feet instead.
const MAX_PARTS: f64 = 1e15;

/// `feet` as feet and inches to the nearest `1/denominator` inch, rounding half away from zero:
/// `12'-5 3/8"`, `1'-0"` (11.95" carries into the next foot), `0'-0 3/8"`. Fractions are
/// reduced (4/8 reads 1/2). A denominator outside 1–64 is taken as 8.
pub fn feet_inches(feet: f64, denominator: u32) -> String {
    let d = if (1..=64).contains(&denominator) { i64::from(denominator) } else { i64::from(EIGHTHS) };
    let parts = (feet.abs() * 12.0 * d as f64).round();
    if !parts.is_finite() || parts > MAX_PARTS {
        return format!("{feet:.2} ft");
    }
    // In range by the check above, so the cast is exact.
    let parts = parts as i64;
    let sign = if feet < 0.0 && parts > 0 { "-" } else { "" };
    let per_foot = 12 * d;
    let (ft, rest) = (parts / per_foot, parts % per_foot);
    let (inches, fraction) = (rest / d, rest % d);
    let fraction = if fraction == 0 {
        String::new()
    } else {
        let g = gcd(fraction, d);
        format!(" {}/{}", fraction / g, d / g)
    };
    format!("{sign}{ft}'-{inches}{fraction}\"")
}

/// A length typed in feet and inches, in feet: `12'-5 3/8"`, `12' 5 3/8"`, `12'5.375"`,
/// `5 3/8"`, `150"`, `12'`, `12.5'`, or a plain decimal number of feet (`12.448`). `′` and `″`
/// are taken for `'` and `"`. Anything else (a fraction without a unit, two numbers without a
/// `'`, a zero denominator) is refused with `None`.
pub fn parse_feet_inches(s: &str) -> Option<f64> {
    let s: String = s
        .trim()
        .chars()
        .map(|c| match c {
            '′' => '\'',
            '″' => '"',
            c => c,
        })
        .collect();
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    let (feet, inches) = match s.split_once('\'') {
        Some((feet, rest)) => {
            // An optional dash or spaces between the feet and the inches.
            let rest = rest.trim().trim_start_matches('-').trim();
            let rest = rest.strip_suffix('"').unwrap_or(rest).trim();
            (decimal(feet.trim())?, if rest.is_empty() { 0.0 } else { mixed(rest)? })
        }
        None => match s.strip_suffix('"') {
            Some(inches) => (0.0, mixed(inches.trim())?),
            None => (decimal(&s)?, 0.0),
        },
    };
    let total = feet + inches / 12.0;
    total.is_finite().then_some(total)
}

/// A plain decimal number, digits with at most one `.` (no sign, exponent or separator).
fn decimal(s: &str) -> Option<f64> {
    let ok = !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit() || c == b'.') && s.bytes().filter(|&c| c == b'.').count() <= 1 && s != ".";
    if ok { s.parse().ok() } else { None }
}

/// Inches: `5`, `5.375`, `3/8` or `5 3/8`.
fn mixed(s: &str) -> Option<f64> {
    let fraction = |f: &str| -> Option<f64> {
        let (n, d) = f.split_once('/')?;
        let (n, d) = (decimal(n.trim())?, decimal(d.trim())?);
        (d > 0.0).then(|| n / d)
    };
    let mut words = s.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(one), None, None) if one.contains('/') => fraction(one),
        (Some(one), None, None) => decimal(one),
        (Some(whole), Some(part), None) if !whole.contains('/') && part.contains('/') => Some(decimal(whole)? + fraction(part)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feet_and_inches_round_to_eighths_and_carry() {
        for (feet, shown) in [
            (12.0 + 5.375 / 12.0, "12'-5 3/8\""),
            (1.0, "1'-0\""),
            // 11.95" is 95.6 eighths, which rounds to 96: a whole foot, not 0'-12".
            (11.95 / 12.0, "1'-0\""),
            (11.9 / 12.0, "0'-11 7/8\""),
            (0.375 / 12.0, "0'-0 3/8\""),
            (6.5 / 12.0, "0'-6 1/2\""),
            (3.25 / 12.0 + 2.0, "2'-3 1/4\""),
            (3.75 / 12.0, "0'-3 3/4\""),
            // A tie rounds away from zero: 0.015625 ft (exact in binary) is 1.5 eighths of an inch.
            (0.015625, "0'-0 1/4\""),
            (0.0, "0'-0\""),
            (-(2.0 + 6.0 / 12.0), "-2'-6\""),
        ] {
            assert_eq!(feet_inches(feet, EIGHTHS), shown, "{feet}");
        }
        assert_eq!(feet_inches(1.0 + 1.0 / 192.0, 16), "1'-0 1/16\"");
        assert_eq!(feet_inches(1e20, 8), "100000000000000000000.00 ft");
        assert_eq!(feet_inches(f64::NAN, 8), "NaN ft");
        assert_eq!(feet_inches(f64::INFINITY, 0), "inf ft");
    }

    #[test]
    fn feet_and_inches_are_read_in_the_usual_ways() {
        let close = |s: &str, feet: f64| {
            let got = parse_feet_inches(s).unwrap_or_else(|| panic!("{s:?} wasn't read"));
            assert!((got - feet).abs() < 1e-12, "{s:?}: {got} != {feet}");
        };
        let twelve_5_3_8 = 12.0 + 5.375 / 12.0;
        for s in ["12'-5 3/8\"", "12' 5 3/8\"", "12'5 3/8\"", "12' - 5 3/8\"", "12'-5.375\"", "12'-5 3/8", "12′-5 3/8″", " 12'-5 3/8\" "] {
            close(s, twelve_5_3_8);
        }
        close("12'", 12.0);
        close("12.5'", 12.5);
        close("5 3/8\"", 5.375 / 12.0);
        close("3/8\"", 0.375 / 12.0);
        close("150\"", 12.5);
        close("12.448", 12.448);
        close("0'-14\"", 14.0 / 12.0);
        for bad in ["", "abc", "5 3/8", "12 5", "-12'", "12'-5 3/0\"", "1e3", "12''", "'5\"", "12'-5 3/8 1/2\"", ".", "1.2.3"] {
            assert_eq!(parse_feet_inches(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn units_convert_through_inches() {
        let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{a} != {b}");
        close(LengthUnit::Millimeters.convert(1.0, LengthUnit::Inches), 25.4);
        close(LengthUnit::Feet.convert(1.0, LengthUnit::Meters), 1.0 / 0.3048);
        close(LengthUnit::FeetInches.convert(36.0, LengthUnit::Inches), 3.0);
        close(LengthUnit::Centimeters.convert(1.0, LengthUnit::Meters), 100.0);
        assert_eq!(LengthUnit::from_token("ft-in"), Some(LengthUnit::FeetInches));
        assert_eq!(LengthUnit::from_label("ft"), Some(LengthUnit::Feet));
        assert_eq!(LengthUnit::from_label("furlong"), None);
    }
}
