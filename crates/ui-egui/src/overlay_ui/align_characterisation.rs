//! OVERLAY.ALIGN.PAGE.1, stage A1 (this fork): what the wizard's alignment does today, measured
//! against known geometry, before the alignment model changes.
//!
//! Each test builds Old and New documents of chosen sheet sizes, opens the wizard on them, sets
//! the sheet choices, and feeds the first pair exact reference points made from a known
//! correspondence `truth` (a point of New's displayed sheet → where it belongs on Old's). It then
//! takes the plan Generate and Preview both use ([`Wizard::plan`]) and measures, for every pair,
//! how far New's sheet corners land from where `truth` says they belong. The points are exact, so
//! a fit reproduces `truth` on the pair it was picked on; what is measured is what happens on the
//! other pairs.
//!
//! A shared alignment is the first pair's fit carried to the other pairs. What "the same
//! alignment" means for a pair with another geometric context (another sheet factor, other sheet
//! sizes) is a modelling choice, so each case is measured under both readings. Both start from
//! the first pair's true correspondence `T₁` and its sheet transform `k₁`, and differ only in how
//! it carries to pair P with sheet transform `k_P`:
//!
//! - **Old-frame reading**: every pair's New is off by the same similarity `e_A` in Old's
//!   displayed frame, after the pair's own sheet scaling: `T₁ = k₁ then e_A`, `T_P = k_P then e_A`.
//! - **New-frame reading**: every New sheet was produced with the same similarity error `e_B` in
//!   its own displayed frame, before its sheet scaling (one plotter, one scanner):
//!   `T₁ = e_B then k₁`, `T_P = e_B then k_P`.
//!
//! On the first pair both readings are `T₁`; where `k_P = k₁` they agree on every pair.

use super::*;

/// A PDF whose pages have these sizes (width, height) in points, each drawing one box.
fn sized(pages: &[(u32, u32)]) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    for (i, (w, h)) in pages.iter().enumerate() {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R >>", 4 + 2 * i));
        let body = "0 g 20 20 40 10 re f";
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The wizard on Old (A, pages `old`) and New (B, pages `new`), pages paired in order. The app is
/// returned too: the wizard's snapshots read its documents.
fn wizard(old: &[(u32, u32)], new: &[(u32, u32)]) -> (PdfCraftApp, Wizard) {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").expect("language");
    // New first, so Old is the second tab and the first other document is New.
    app.open_bytes("new.pdf", None, sized(new)).expect("New opens");
    app.open_bytes("old.pdf", None, sized(old)).expect("Old opens");
    let old_id = app.views.iter().map(|v| v.id).nth(1).expect("Old's tab");
    let w = Wizard::new(&app, old_id);
    assert_eq!(w.b.as_ref().map(|b| b.name.as_str()), Some("new.pdf"));
    assert_eq!(w.matched_rows().len(), old.len().min(new.len()));
    (app, w)
}

/// A similarity: turn by `degrees` about the origin, scale by `scale`, then shift by (`tx`, `ty`).
fn similarity(degrees: f64, scale: f64, tx: f64, ty: f64) -> Matrix {
    let (s, c) = degrees.to_radians().sin_cos();
    Matrix([scale * c, scale * s, -scale * s, scale * c, tx, ty])
}

/// The error every case uses: a 0.3° turn, 0.2 % larger, 20 pt right and 7 pt up. Realistic for
/// a reprint or a scan, and large enough to measure.
fn error() -> Matrix {
    similarity(0.3, 1.002, 20.0, 7.0)
}

/// The sheet transform the wizard applies to `pair` (`k`): `sheet` for those sheet sizes.
fn k_of(w: &mut Wizard, pair: (usize, usize)) -> Matrix {
    let (sa, sb) = w.sheet_sizes(pair).expect("sheet sizes");
    w.sheet_match(pair).transform(sa, sb).expect("sheet transform")
}

/// Three well-spread points on `pair`, New's at fixed places of its sheet and Old's where
/// `truth` puts them: Old's three first, then New's, as the wizard asks.
fn picks_for(w: &mut Wizard, pair: (usize, usize), truth: &Matrix) -> Picks {
    let (_, (wb, hb)) = w.sheet_sizes(pair).expect("sheet sizes");
    let on_new = [(0.1 * wb, 0.1 * hb), (0.9 * wb, 0.15 * hb), (0.5 * wb, 0.9 * hb)];
    let mut picks = Picks::default();
    for &(x, y) in &on_new {
        let (ax, ay) = truth.apply(x, y);
        assert!(picks.click(Side::A, Point::new(ax, ay), Side::A));
    }
    for &(x, y) in &on_new {
        assert!(picks.click(Side::B, Point::new(x, y), Side::A));
    }
    assert_eq!(picks.pairs().len(), 3);
    picks
}

/// Pick the main alignment on `pair` (what the wizard calls the first pair's alignment before
/// OVERLAY.ALIGN.PAGE.1: the default for every pair).
fn pick(w: &mut Wizard, pair: (usize, usize), truth: &Matrix) {
    let picks = picks_for(w, pair, truth);
    w.set_picks(Target::Main, picks, pair);
}

/// Tick `pair` and pick its own alignment.
fn pick_own(w: &mut Wizard, pair: (usize, usize), truth: &Matrix) {
    let picks = picks_for(w, pair, truth);
    w.tick(pair);
    w.set_picks(Target::Own(pair), picks, pair);
}

/// Confirm `choice` for `pair`'s sheet size (or for every pair while it is shared).
fn choose(w: &mut Wizard, pair: (usize, usize), choice: SheetChoice) {
    w.sheets.insert(pair, SheetState { choice, confirmed: true });
}

/// For each matched row, the farthest New's sheet corners land from where `truth(pair)` puts
/// them, in points, under the plan Generate uses.
fn misplacement(w: &mut Wizard, truth: impl Fn(&mut Wizard, (usize, usize)) -> Matrix) -> Vec<f64> {
    let rows = w.pairs.clone();
    let plan = w.plan(&rows).expect("the plan");
    let mut out = Vec::new();
    for (row, pp) in rows.iter().zip(&plan.pairs) {
        let Some(pair) = row.pages() else { continue };
        let (sa, sb) = w.sheet_sizes(pair).expect("sheet sizes");
        let placed = pp.sheet.transform(sa, sb).expect("sheet transform").then(&pp.registration);
        let want = truth(w, pair);
        let (wb, hb) = sb;
        let worst = [(0.0, 0.0), (wb, 0.0), (wb, hb), (0.0, hb)]
            .iter()
            .map(|&(x, y)| {
                let (px, py) = placed.apply(x, y);
                let (tx, ty) = want.apply(x, y);
                (px - tx).hypot(py - ty)
            })
            .fold(0.0, f64::max);
        out.push(worst);
    }
    out
}

/// The first pair's true correspondence: New's sheet scaled as the wizard scales it, then off by
/// [`error`] in Old's frame. Any similarity would do: both readings start from it.
fn anchor(w: &mut Wizard, first: (usize, usize)) -> Matrix {
    k_of(w, first).then(&error())
}

/// The two readings of "the same alignment" on every pair, from the first pair's truth `t1`.
struct Readings {
    /// `e_A`: the error in Old's frame, after the sheet scaling.
    old: Matrix,
    /// `e_B`: the error in New's frame, before the sheet scaling.
    new: Matrix,
}

impl Readings {
    fn of(w: &mut Wizard, first: (usize, usize), t1: &Matrix) -> Readings {
        let k1 = k_of(w, first);
        let inv = k1.invert().expect("the first sheet transform inverts");
        Readings { old: inv.then(t1), new: t1.then(&inv) }
    }

    fn old_frame(&self) -> impl Fn(&mut Wizard, (usize, usize)) -> Matrix + '_ {
        |w, pair| k_of(w, pair).then(&self.old)
    }

    fn new_frame(&self) -> impl Fn(&mut Wizard, (usize, usize)) -> Matrix + '_ {
        |w, pair| self.new.then(&k_of(w, pair))
    }

    /// Misplacement on every pair under (Old-frame, New-frame) readings.
    fn measure(&self, w: &mut Wizard) -> (Vec<f64>, Vec<f64>) {
        (misplacement(w, self.old_frame()), misplacement(w, self.new_frame()))
    }
}

/// Pick the first pair from its truth and give both readings.
fn pick_first(w: &mut Wizard) -> Readings {
    let first = w.first_pair().expect("a pair");
    let t1 = anchor(w, first);
    pick(w, first, &t1);
    Readings::of(w, first, &t1)
}

const EXACT: f64 = 1e-6;

fn mm(pt: f64) -> f64 {
    pt / PT_PER_MM
}

/// Equal sheets on every pair (1:1, nothing to confirm): the first pair's alignment is exact on
/// every pair, under either reading. The case the shared alignment is made for.
#[test]
fn equal_sheets_share_the_alignment_exactly() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200), (300, 200)], &[(300, 200), (300, 200), (300, 200)]);
    let r = pick_first(&mut w);
    let (old, new) = r.measure(&mut w);
    for m in [old, new] {
        assert_eq!(m.len(), 3);
        assert!(m.iter().all(|e| *e < EXACT), "{m:?}");
    }
}

/// Every New twice its Old's size and one shared factor, 0.5: every pair has the same sheet
/// transform, so the shared alignment is exact on every pair, under either reading.
#[test]
fn a_shared_factor_on_pairs_of_the_same_sizes_is_exact() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200)], &[(600, 400), (600, 400)]);
    let first = w.first_pair().expect("a pair");
    choose(&mut w, first, SheetChoice::Factor(0.5));
    assert!(w.same_sheet_for_all && w.own.is_empty());
    let r = pick_first(&mut w);
    let (old, new) = r.measure(&mut w);
    for m in [old, new] {
        assert!(m.iter().all(|e| *e < EXACT), "{m:?}");
    }
}

/// Match sheet sizes off: every pair is 1:1 at its lower-left corner whatever its sizes, so the
/// sheet transform is the identity everywhere and the shared alignment is exact on every pair.
#[test]
fn matching_off_over_pairs_of_other_sizes_is_exact() {
    let (_app, mut w) = wizard(&[(300, 200), (420, 297)], &[(300, 200), (420, 297)]);
    w.match_sheets = false;
    let r = pick_first(&mut w);
    let (old, new) = r.measure(&mut w);
    for m in [old, new] {
        assert!(m.iter().all(|e| *e < EXACT), "{m:?}");
    }
}

/// One shared factor (0.5) over pairs whose sheets are other sizes but the same proportion: each
/// pair's sheet transform scales by 0.5 with no centring shift, so the shared alignment is still
/// exact on every pair, under either reading.
#[test]
fn a_shared_factor_over_proportional_pairs_of_other_sizes_is_exact() {
    let (_app, mut w) = wizard(&[(300, 200), (420, 280)], &[(600, 400), (840, 560)]);
    let first = w.first_pair().expect("a pair");
    choose(&mut w, first, SheetChoice::Factor(0.5));
    let r = pick_first(&mut w);
    let (old, new) = r.measure(&mut w);
    for m in [old, new] {
        assert!(m.iter().all(|e| *e < EXACT), "{m:?}");
    }
}

/// One shared factor over a pair whose New isn't exactly twice its Old: the factor's centring
/// moves that pair's New by (Old − factor × New) / 2. Under the Old-frame reading the alignment
/// is exact; under the New-frame reading the turn and scale of the error act about another
/// centre, which misplaces it by (L − I)·Δc, small here (a 3 pt centring difference).
#[test]
fn a_shared_factor_over_a_pair_off_its_proportion_shifts_the_centre_slightly() {
    let (_app, mut w) = wizard(&[(300, 200), (420, 297)], &[(600, 400), (840, 600)]);
    let first = w.first_pair().expect("a pair");
    choose(&mut w, first, SheetChoice::Factor(0.5));
    let r = pick_first(&mut w);
    let (old, new) = r.measure(&mut w);
    assert!(old.iter().all(|e| *e < EXACT), "{old:?}");
    assert!(new[0] < EXACT, "{new:?}");
    eprintln!("shared factor, pair off its proportion, New-frame reading: second pair misplaced by {:.4} pt", new[1]);
    // |L − I| ≈ 0.0058 for a 0.3° turn and 0.2 % scale; Δc = (0, 1.5) pt.
    assert!(new[1] > EXACT && new[1] < 0.02, "{new:?}");
}

/// The configuration where the readings part: the sheet choice per pair (the first pair's New is
/// twice its Old, factor 0.5; the second pair's sheets are equal, 1:1), with the alignment still
/// shared. The first pair's fit is made after its 0.5 scale and is applied to the second pair
/// after a 1:1 one.
///
/// - Old-frame reading: exact.
/// - New-frame reading: the first pair's error, measured on Old after New was halved, is twice as
///   large in New's own frame; the second pair's New is at full scale, so it needs that larger
///   correction and gets the first pair's: misplaced by 21.2 pt (7.5 mm) at its corners, the
///   size of the whole error. No warning is shown and Generate goes ahead.
#[test]
fn per_pair_factors_carry_the_first_pairs_fit_into_another_scale() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200)], &[(600, 400), (300, 200)]);
    w.same_sheet_for_all = false;
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    choose(&mut w, pairs[0], SheetChoice::Factor(0.5));
    // The second pair's sheets are the same size: 1:1, confirmed by itself.
    assert_eq!(w.sheet_match(pairs[1]), SheetMatch::Off);
    assert_eq!(w.sheet_match(pairs[0]), SheetMatch::Scaled { factor: 0.5 });
    // Its sheets differ, so the first pair needs its own OV6.9 override, as the user ticks it.
    w.allow_different.insert(pairs[0]);
    let r = pick_first(&mut w);
    // Nothing stops it: the step has no problem to report.
    assert_eq!(w.sheets_problem(), None);
    assert_eq!(w.sheet_size_problem(), None);
    assert_eq!(w.alignment_problem(), None);
    // The second pair is given the first pair's registration matrix, unchanged.
    let r0 = w.registration(pairs[0]).expect("first").matrix;
    let r1 = w.registration(pairs[1]).expect("second").matrix;
    assert_eq!(r0, r1);
    let (old, new) = r.measure(&mut w);
    assert!(old.iter().all(|e| *e < EXACT), "{old:?}");
    assert!(new[0] < EXACT, "{new:?}");
    eprintln!("per-pair factors, New-frame reading: second pair misplaced by {:.3} pt ({:.3} mm)", new[1], mm(new[1]));
    assert!((new[1] - 21.19).abs() < 0.05, "{new:?}");
}

/// Two different factors (0.5 and 0.25), alignment shared: the same, scaled. The second pair's
/// fit is measured at 0.5 and applied at 0.25.
#[test]
fn two_different_factors_carry_the_fit_at_the_wrong_scale() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200)], &[(600, 400), (1200, 800)]);
    w.same_sheet_for_all = false;
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    choose(&mut w, pairs[0], SheetChoice::Factor(0.5));
    choose(&mut w, pairs[1], SheetChoice::Factor(0.25));
    // Both pairs' sheets differ: each needs its own OV6.9 override; the shared points satisfy the
    // second pair's need for reference points.
    w.allow_different.insert(pairs[0]);
    w.allow_different.insert(pairs[1]);
    let r = pick_first(&mut w);
    assert_eq!(w.sheets_problem(), None);
    assert_eq!(w.sheet_size_problem(), None);
    assert_eq!(w.alignment_problem(), None);
    let (old, new) = r.measure(&mut w);
    assert!(old.iter().all(|e| *e < EXACT), "{old:?}");
    eprintln!("factors 0.5 and 0.25, New-frame reading: second pair misplaced by {:.3} pt ({:.3} mm)", new[1], mm(new[1]));
    assert!(new[0] < EXACT && new[1] > 4.0, "{new:?}");
}

/// Each pair with its own alignment (both ticked): each gets its own matrix, exact on its own
/// geometry. (The test the wizard did not have.)
#[test]
fn separate_alignments_give_each_pair_its_own_exact_matrix() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200)], &[(600, 400), (300, 200)]);
    w.same_sheet_for_all = false;
    let pairs: Vec<(usize, usize)> = w.matched_rows().into_iter().map(|(_, p)| p).collect();
    choose(&mut w, pairs[0], SheetChoice::Factor(0.5));
    // Each pair picked from its own truth under the New-frame reading (the one a shared
    // alignment gets wrong here).
    let first = pairs[0];
    let t1 = anchor(&mut w, first);
    let r = Readings::of(&mut w, first, &t1);
    for &pair in &pairs {
        let truth = r.new_frame()(&mut w, pair);
        pick_own(&mut w, pair, &truth);
    }
    let r0 = w.registration(pairs[0]).expect("first").matrix;
    let r1 = w.registration(pairs[1]).expect("second").matrix;
    assert_ne!(r0, r1, "each pair its own matrix");
    let (_, new) = r.measure(&mut w);
    assert!(new.iter().all(|e| *e < EXACT), "{new:?}");
    // Its own alignment removed, with no main alignment: placed by its sheet match alone (the
    // identity), never another pair's own fit.
    w.remove_own(pairs[1]);
    assert_eq!(w.registration(pairs[1]).expect("second").matrix, Matrix::IDENTITY);
}

/// The preview of one pair and the generated page use the same registration: both come from
/// [`Wizard::pair_plan`].
#[test]
fn the_preview_and_the_output_use_the_same_plan_for_a_pair() {
    let (_app, mut w) = wizard(&[(300, 200), (300, 200)], &[(300, 200), (300, 200)]);
    let _ = pick_first(&mut w);
    let rows = w.pairs.clone();
    let all = w.plan(&rows).expect("the plan");
    for (i, row) in rows.iter().enumerate() {
        let one = w.plan(std::slice::from_ref(row)).expect("one pair's plan");
        assert_eq!(one.pairs[0].registration, all.pairs[i].registration);
        assert_eq!(one.pairs[0].sheet, all.pairs[i].sheet);
    }
}
