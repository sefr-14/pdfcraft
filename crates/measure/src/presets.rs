//! Common drawing scales (this fork, sefr-14/pdfcraft, MEASURE.2): the metric ratios and imperial
//! architect's and engineer's scales offered by the measure panel and the `measure_scale`
//! automation tool, and how a "1:N" ratio is read and written.

/// The metric drawing scales offered, as the N of 1:N (the owner's table of common view scales).
pub const METRIC: [f64; 13] = [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0];
/// An imperial drawing scale as it is written on a plan: `paper` inches on the drawing are `real`
/// feet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Imperial {
    pub label: &'static str,
    pub paper: f64,
    pub real: f64,
}
impl Imperial {
    /// The N of 1:N: real inches per paper inch.
    pub fn ratio(self) -> f64 {
        self.real * 12.0 / self.paper
    }
}
const fn imperial(label: &'static str, paper: f64, real: f64) -> Imperial {
    Imperial { label, paper, real }
}
/// The imperial drawing scales offered, largest first (the owner's table). Every paper
/// length is a binary fraction of an inch, so each ratio is exact.
pub const IMPERIAL: [Imperial; 28] = [
    imperial("12\" = 1'-0\"", 12.0, 1.0),
    imperial("6\" = 1'-0\"", 6.0, 1.0),
    imperial("3\" = 1'-0\"", 3.0, 1.0),
    imperial("1 1/2\" = 1'-0\"", 1.5, 1.0),
    imperial("1\" = 1'-0\"", 1.0, 1.0),
    imperial("3/4\" = 1'-0\"", 0.75, 1.0),
    imperial("1/2\" = 1'-0\"", 0.5, 1.0),
    imperial("3/8\" = 1'-0\"", 0.375, 1.0),
    imperial("1/4\" = 1'-0\"", 0.25, 1.0),
    imperial("3/16\" = 1'-0\"", 0.1875, 1.0),
    imperial("1/8\" = 1'-0\"", 0.125, 1.0),
    imperial("1\" = 10'-0\"", 1.0, 10.0),
    imperial("3/32\" = 1'-0\"", 0.09375, 1.0),
    imperial("1/16\" = 1'-0\"", 0.0625, 1.0),
    imperial("1\" = 20'-0\"", 1.0, 20.0),
    imperial("3/64\" = 1'-0\"", 0.046875, 1.0),
    imperial("1\" = 30'-0\"", 1.0, 30.0),
    imperial("1/32\" = 1'-0\"", 0.03125, 1.0),
    imperial("1\" = 40'-0\"", 1.0, 40.0),
    imperial("1\" = 50'-0\"", 1.0, 50.0),
    imperial("1\" = 60'-0\"", 1.0, 60.0),
    imperial("1/64\" = 1'-0\"", 0.015625, 1.0),
    imperial("1\" = 80'-0\"", 1.0, 80.0),
    imperial("1\" = 100'-0\"", 1.0, 100.0),
    imperial("1\" = 160'-0\"", 1.0, 160.0),
    imperial("1\" = 200'-0\"", 1.0, 200.0),
    imperial("1\" = 300'-0\"", 1.0, 300.0),
    imperial("1\" = 400'-0\"", 1.0, 400.0),
];
/// "1:200", "1/200", "1 : 200" or a bare "200" (a comma works as the decimal mark), as the N of 1:N.
/// `None` for anything else, for zero, negative or absurd values.
pub fn parse_ratio(text: &str) -> Option<f64> {
    let text = text.trim().replace(',', ".");
    let n = match text.split_once([':', '/']) {
        Some((one, n)) => n.trim().parse::<f64>().ok()? / one.trim().parse::<f64>().ok()?,
        None => text.parse::<f64>().ok()?,
    };
    RATIO_RANGE.contains(&n).then_some(n)
}
/// The N of 1:N accepted. The lower bound keeps a real distance of N / 1000 m within the measure
/// panel's field minimum (0.000001).
pub const RATIO_RANGE: std::ops::RangeInclusive<f64> = 1e-3..=1e9;
/// "1:200", or "1:2.5" for a ratio that isn't a whole number: the number as it is, so the field shows exactly what applies.
pub fn format_ratio(n: f64) -> String {
    format!("1:{n}")
}

/// The imperial scale written `label` (`1/4" = 1'-0"`), if it is one of [`IMPERIAL`].
pub fn imperial_named(label: &str) -> Option<Imperial> {
    let label = label.trim();
    IMPERIAL.iter().copied().find(|p| p.label == label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner's table: each imperial scale's 1:N.
    #[test]
    fn every_imperial_scale_has_the_ratio_of_the_table() {
        let table = [
            1.0, 2.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0, 96.0, 120.0, 128.0, 192.0, 240.0, 256.0, 360.0, 384.0, 480.0, 600.0, 720.0,
            768.0, 960.0, 1200.0, 1920.0, 2400.0, 3600.0, 4800.0,
        ];
        assert_eq!(IMPERIAL.len(), table.len());
        for (preset, n) in IMPERIAL.iter().zip(table) {
            // Exact: every paper length is a binary fraction of an inch.
            assert_eq!(preset.ratio(), n, "{}", preset.label);
            assert_eq!(imperial_named(preset.label), Some(*preset));
        }
        assert_eq!(imperial_named("1/4\" = 1'-0\"").map(Imperial::ratio), Some(48.0));
        assert_eq!(imperial_named("1/8\" = 1'-0\"").map(Imperial::ratio), Some(96.0));
        assert_eq!(imperial_named("1:48"), None);
    }

    #[test]
    fn every_metric_scale_reads_back() {
        assert_eq!(METRIC, [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0]);
        for n in METRIC {
            assert_eq!(parse_ratio(&format_ratio(n)), Some(n));
        }
    }
}
