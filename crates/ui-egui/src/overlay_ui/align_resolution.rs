//! OVERLAY.ALIGN.PAGE.1 (this fork): which alignment each pair is placed with. The main alignment
//! is the default for every matched pair; pairs ticked on the checklist get their own, used only
//! when it is complete, valid and current; a stale main alignment blocks exactly the pairs that
//! would use it; points never follow a pair whose match changed.

use super::*;

/// A PDF of `n` pages, each `w` × `h` points, each drawing one box.
fn pages(n: usize, w: u32, h: u32) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    for i in 0..n {
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

/// The wizard on three-page Old and New documents (`old` and `new` sheet sizes).
fn wizard_sized(old: (u32, u32), new: (u32, u32)) -> (PdfCraftApp, Wizard) {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").expect("language");
    app.open_bytes("new.pdf", None, pages(3, new.0, new.1)).expect("New opens");
    app.open_bytes("old.pdf", None, pages(3, old.0, old.1)).expect("Old opens");
    let old_id = app.views.iter().map(|v| v.id).nth(1).expect("Old's tab");
    let w = Wizard::new(&app, old_id);
    assert_eq!(w.matched_rows().len(), 3);
    (app, w)
}

fn wizard() -> (PdfCraftApp, Wizard) {
    wizard_sized((300, 200), (300, 200))
}

/// Three exact points that move New by (`dx`, `dy`) onto Old.
fn shift(dx: f64, dy: f64) -> Picks {
    let mut p = Picks::default();
    let on_new = [(30.0, 20.0), (270.0, 30.0), (150.0, 180.0)];
    for (x, y) in on_new {
        assert!(p.click(Side::A, Point::new(x + dx, y + dy), Side::A));
    }
    for (x, y) in on_new {
        assert!(p.click(Side::B, Point::new(x, y), Side::A));
    }
    p
}

/// Three points on a line: the engine refuses them.
fn collinear() -> Picks {
    let mut p = Picks::default();
    for x in [10.0, 20.0, 30.0] {
        assert!(p.click(Side::A, Point::new(x, x), Side::A));
    }
    for x in [10.0, 20.0, 30.0] {
        assert!(p.click(Side::B, Point::new(x, x), Side::A));
    }
    p
}

fn rows(w: &Wizard) -> Vec<PairKey> {
    w.matched_rows().into_iter().map(|(_, p)| p).collect()
}

fn translation(w: &mut Wizard, pair: PairKey) -> Option<(f64, f64)> {
    w.registration(pair).ok().map(|r| (r.matrix.0[4], r.matrix.0[5]))
}

fn status(w: &mut Wizard, pair: PairKey) -> Result<PairStatus, String> {
    w.effective(pair).map(|(_, s)| s)
}

fn close(a: Option<(f64, f64)>, b: (f64, f64)) -> bool {
    a.is_some_and(|a| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6)
}

/// A document refreshed in its tab (a new snapshot): what the stale banner's "Use current"
/// does, simulated by a new edit generation.
fn refresh_new(w: &mut Wizard) {
    if let Some(b) = w.b.as_mut() {
        b.generation = b.generation.wrapping_add(1);
    }
}

#[test]
fn without_a_main_alignment_every_pair_is_placed_by_its_sheet_match() {
    let (_app, mut w) = wizard();
    for p in rows(&w) {
        assert_eq!(status(&mut w, p), Ok(PairStatus::Placed));
        assert_eq!(w.registration(p).expect("placed").matrix, Matrix::IDENTITY);
    }
    assert_eq!(w.alignment_problem(), None);
}

#[test]
fn every_unticked_pair_uses_the_main_alignment() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    assert_eq!(w.main_state(), MainState::Current);
    for p in r {
        assert_eq!(status(&mut w, p), Ok(PairStatus::Main));
        assert!(close(translation(&mut w, p), (10.0, 5.0)));
        assert!(!w.registration(p).expect("main").note.own);
    }
    assert!(w.own.is_empty(), "nothing is ticked for the user");
}

#[test]
fn a_ticked_pair_with_a_complete_valid_alignment_uses_it_and_only_it() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    w.tick(r[1]);
    w.set_picks(Target::Own(r[1]), shift(-4.0, 2.0), r[1]);
    assert_eq!(status(&mut w, r[1]), Ok(PairStatus::Own));
    assert!(close(translation(&mut w, r[1]), (-4.0, 2.0)));
    assert!(w.registration(r[1]).expect("own").note.own, "the output says which pages used their own");
    for p in [r[0], r[2]] {
        assert_eq!(status(&mut w, p), Ok(PairStatus::Main));
        assert!(close(translation(&mut w, p), (10.0, 5.0)));
    }
}

#[test]
fn an_own_alignment_not_finished_or_not_valid_leaves_the_pair_on_the_main_one() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    // Ticked, no points: pending.
    w.tick(r[1]);
    assert_eq!(status(&mut w, r[1]), Ok(PairStatus::OwnPending));
    assert!(close(translation(&mut w, r[1]), (10.0, 5.0)));
    // Two Old points only: a round under way, still pending.
    let mut half = Picks::default();
    for x in [30.0, 270.0] {
        assert!(half.click(Side::A, Point::new(x, 20.0), Side::A));
    }
    w.set_picks(Target::Own(r[1]), half, r[1]);
    assert_eq!(status(&mut w, r[1]), Ok(PairStatus::OwnPending));
    assert!(close(translation(&mut w, r[1]), (10.0, 5.0)));
    // Points the engine refuses: not valid, with the reason, and the main alignment is used.
    w.set_picks(Target::Own(r[1]), collinear(), r[1]);
    assert!(matches!(status(&mut w, r[1]), Ok(PairStatus::OwnInvalid(_))), "{:?}", status(&mut w, r[1]));
    assert!(close(translation(&mut w, r[1]), (10.0, 5.0)));
    assert!(w.status_text(r[1]).contains("main alignment is used"));
    // None of this blocks the step.
    assert_eq!(w.alignment_problem(), None);
}

#[test]
fn unticking_or_removing_returns_the_pair_to_the_main_alignment() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    w.tick(r[2]);
    w.set_picks(Target::Own(r[2]), shift(-4.0, 2.0), r[2]);
    w.untick(r[2]);
    assert_eq!(status(&mut w, r[2]), Ok(PairStatus::Main));
    assert!(close(translation(&mut w, r[2]), (10.0, 5.0)));
    // Ticked again: its points come back.
    w.tick(r[2]);
    assert_eq!(status(&mut w, r[2]), Ok(PairStatus::Own));
    assert!(close(translation(&mut w, r[2]), (-4.0, 2.0)));
    // Removed: gone, and ticking again starts empty.
    w.remove_own(r[2]);
    assert_eq!(status(&mut w, r[2]), Ok(PairStatus::Main));
    w.tick(r[2]);
    assert_eq!(status(&mut w, r[2]), Ok(PairStatus::OwnPending));
}

/// Editing one pair's own alignment leaves the main alignment and every other pair's own exactly
/// as they were.
#[test]
fn an_override_belongs_to_its_pair_alone() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    w.tick(r[1]);
    w.set_picks(Target::Own(r[1]), shift(-4.0, 2.0), r[1]);
    w.tick(r[2]);
    w.set_picks(Target::Own(r[2]), shift(1.0, 1.0), r[2]);
    let (main, other) = (w.main.clone(), w.own.get(&r[2]).cloned());
    let m0 = w.registration(r[0]).expect("main").matrix;
    let m2 = w.registration(r[2]).expect("own").matrix;
    w.set_picks(Target::Own(r[1]), shift(7.0, -3.0), r[1]);
    assert_eq!(w.main, main);
    assert_eq!(w.own.get(&r[2]).cloned(), other);
    assert_eq!(w.registration(r[0]).expect("main").matrix, m0);
    assert_eq!(w.registration(r[2]).expect("own").matrix, m2);
    assert!(close(translation(&mut w, r[1]), (7.0, -3.0)));
}

/// A document refreshed after the points were picked: the main alignment blocks exactly the
/// pairs that would use it, and a pair with a valid, current own alignment goes on.
#[test]
fn a_stale_main_alignment_blocks_only_the_pairs_that_would_use_it() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    w.tick(r[1]);
    w.set_picks(Target::Own(r[1]), shift(-4.0, 2.0), r[1]);
    w.tick(r[2]);
    refresh_new(&mut w);
    assert_eq!(w.main_state(), MainState::Refreshed);
    // Unticked: uses main, blocked. Ticked but stale or unfinished: would fall back on main,
    // blocked.
    assert!(w.effective(r[0]).is_err());
    assert!(w.effective(r[1]).is_err(), "its own is stale too, and main can't stand in");
    assert!(w.effective(r[2]).is_err());
    assert!(w.alignment_problem().is_some());
    // The own alignment confirmed: that pair goes on, the others stay blocked.
    w.own.get_mut(&r[1]).expect("own").stamp = w.stamp();
    assert_eq!(status(&mut w, r[1]), Ok(PairStatus::Own));
    assert!(w.effective(r[0]).is_err());
    // Every pair with a valid, current own alignment: the stale main alignment blocks nothing.
    for p in [r[0], r[2]] {
        w.tick(p);
        w.set_picks(Target::Own(p), shift(2.0, 2.0), p);
    }
    assert_eq!(w.main_state(), MainState::Refreshed);
    assert_eq!(w.alignment_problem(), None);
    // Confirming the main alignment unblocks the rest as well.
    w.main.stamp = w.stamp();
    assert_eq!(w.main_state(), MainState::Current);
}

/// With no main alignment, a stale own alignment falls back on the sheet match, which is never
/// stale.
#[test]
fn a_stale_own_alignment_without_a_main_one_is_placed_by_its_sheet_match() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.tick(r[1]);
    w.set_picks(Target::Own(r[1]), shift(-4.0, 2.0), r[1]);
    refresh_new(&mut w);
    assert_eq!(status(&mut w, r[1]), Ok(PairStatus::OwnStale));
    assert_eq!(w.registration(r[1]).expect("placed").matrix, Matrix::IDENTITY);
    assert!(w.status_text(r[1]).contains("needs review"));
}

/// Drawing the step never confirms stale points: only a real change restamps them.
#[test]
fn storing_the_same_points_does_not_confirm_them() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    refresh_new(&mut w);
    let same = w.picks_of(Target::Main);
    w.set_picks(Target::Main, same, r[0]);
    assert_eq!(w.main_state(), MainState::Refreshed);
}

/// O-A6: a pair's own alignment is set aside when its match changes, never applied to the new
/// pair, and comes back with the old match against the same snapshots, unfinished ones too.
#[test]
fn a_rematch_sets_own_alignments_aside_and_the_same_match_brings_them_back() {
    let (_app, mut w) = wizard();
    w.set_picks(Target::Main, shift(10.0, 5.0), (0, 0));
    w.tick((1, 1));
    w.set_picks(Target::Own((1, 1)), shift(-4.0, 2.0), (1, 1));
    w.tick((2, 2));
    // Old 2 now matched with New 3 and Old 3 with New 2.
    w.pairing = Pairing::in_order(&[0, 1, 2], &[0, 2, 1]);
    w.sync_pairs();
    assert!(w.own.is_empty());
    assert!(w.parked.contains_key(&(1, 1)) && w.parked.contains_key(&(2, 2)));
    for p in [(1, 2), (2, 1)] {
        assert_eq!(status(&mut w, p), Ok(PairStatus::Main), "the new pairs use the main alignment");
    }
    // The match restored: both come back as they were, the unfinished one unfinished.
    w.pairing = Pairing::in_order(&[0, 1, 2], &[0, 1, 2]);
    w.sync_pairs();
    assert_eq!(status(&mut w, (1, 1)), Ok(PairStatus::Own));
    assert!(close(translation(&mut w, (1, 1)), (-4.0, 2.0)));
    assert_eq!(status(&mut w, (2, 2)), Ok(PairStatus::OwnPending));
    // Set aside, then a document refreshed: the same match no longer brings it back.
    w.pairing = Pairing::in_order(&[0, 1, 2], &[0, 2, 1]);
    w.sync_pairs();
    refresh_new(&mut w);
    w.pairing = Pairing::in_order(&[0, 1, 2], &[0, 1, 2]);
    w.sync_pairs();
    assert!(!w.own.contains_key(&(1, 1)), "points of another version stay aside");
    assert!(w.parked.contains_key(&(1, 1)));
    // The unfinished one had no points: nothing to mismatch, it comes back.
    assert!(w.own.contains_key(&(2, 2)));
}

/// The pair the main alignment was picked on re-paired: pairs that would use it are blocked
/// (its points belong to that pair's sheets); restoring the match brings it back.
#[test]
fn the_main_alignments_reference_pair_rematched_blocks_until_restored_or_cleared() {
    let (_app, mut w) = wizard();
    w.set_picks(Target::Main, shift(10.0, 5.0), (0, 0));
    w.tick((2, 2));
    w.set_picks(Target::Own((2, 2)), shift(1.0, 1.0), (2, 2));
    w.pairing = Pairing::in_order(&[0, 1, 2], &[1, 0, 2]);
    w.sync_pairs();
    assert_eq!(w.main_state(), MainState::ReferenceGone);
    assert!(w.effective((0, 1)).is_err() && w.effective((1, 0)).is_err());
    assert_eq!(status(&mut w, (2, 2)), Ok(PairStatus::Own), "a pair with its own goes on");
    w.pairing = Pairing::in_order(&[0, 1, 2], &[0, 1, 2]);
    w.sync_pairs();
    assert_eq!(w.main_state(), MainState::Current);
    assert_eq!(w.alignment_problem(), None);
    // Cleared instead: every pair without its own is placed by its sheet match.
    w.pairing = Pairing::in_order(&[0, 1, 2], &[1, 0, 2]);
    w.sync_pairs();
    w.main = MainAlign::default();
    assert_eq!(status(&mut w, (0, 1)), Ok(PairStatus::Placed));
}

/// Different sheet sizes put no pair on the checklist by themselves; OV6.9 still asks for its
/// override and reference points, which the main alignment's points satisfy, as before.
#[test]
fn different_sheet_sizes_tick_nothing_and_ov6_9_is_unchanged() {
    let (_app, mut w) = wizard_sized((300, 200), (600, 400));
    w.match_sheets = false;
    assert!(w.own.is_empty());
    let r = rows(&w);
    assert!(w.sheet_size_problem().is_some(), "not allowed yet");
    for p in &r {
        w.allow_different.insert(*p);
    }
    assert!(w.sheet_size_problem().is_some(), "no reference points yet");
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    assert_eq!(w.sheet_size_problem(), None, "the main alignment's points serve every pair");
    assert!(w.own.is_empty());
}

/// Preview of one pair and the full plan place a pair with the same registration, its own or
/// the main one.
#[test]
fn preview_and_generate_place_each_pair_alike() {
    let (_app, mut w) = wizard();
    let r = rows(&w);
    w.set_picks(Target::Main, shift(10.0, 5.0), r[0]);
    w.tick(r[1]);
    w.set_picks(Target::Own(r[1]), shift(-4.0, 2.0), r[1]);
    w.tick(r[2]);
    let all_rows = w.pairs.clone();
    let all = w.plan(&all_rows).expect("plan");
    for (i, row) in all_rows.iter().enumerate() {
        let one = w.plan(std::slice::from_ref(row)).expect("one");
        assert_eq!(one.pairs[0].registration, all.pairs[i].registration);
        assert_eq!(one.pairs[0].note, all.pairs[i].note);
    }
    assert!(all.pairs[1].note.own && !all.pairs[0].note.own && !all.pairs[2].note.own);
}
