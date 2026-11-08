//! This fork (HOME.1): Home recommends Organize pages, Combine files, Compare files (with Overlay),
//! Edit a PDF and Protect a PDF. Each card opens its existing tool panel, except Combine files,
//! which opens the Combine tab itself (COMBINE.1); comments and forms stay in All tools.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{FilePurpose, LeftPanel, PdfCraftApp};

fn harness(language: &'static str) -> Harness<'static, PdfCraftApp> {
    harness_with(language, |_| {})
}

fn harness_with(language: &'static str, setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", language).unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

/// A one-page PDF.
fn one_page() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", "<< /Type /Page /Parent 2 0 R >>"];
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

/// Click the Home card labelled `label` (the 190-point-wide one, not an All tools row).
fn click_card(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    h.query_all_by_label(label).find(|n| (n.rect().width() - 190.0).abs() < 1.0).unwrap_or_else(|| panic!("{label} card")).click();
    h.run_steps(3);
}

/// Combine files is showing, with an empty list and its empty-state controls, and no tool pane
/// was opened on the way.
fn assert_empty_combine(h: &Harness<'static, PdfCraftApp>) {
    let app = h.state();
    assert!(app.combine_showing(), "the Combine tab shows");
    assert!(app.combine_draft.is_empty(), "with an empty list");
    assert_ne!(app.left, LeftPanel::Tool("combine"), "not the Combine tool pane");
    h.get_by_label("Add the files to combine");
}

/// The Home cards labelled `label` (190 × 104 points), told apart from the All tools list by size.
fn cards(h: &Harness<'static, PdfCraftApp>, label: &str) -> usize {
    h.query_all_by_label(label).filter(|n| (n.rect().width() - 190.0).abs() < 1.0 && (n.rect().height() - 104.0).abs() < 1.0).count()
}

#[test]
fn home_recommends_combine_and_compare_instead_of_comments_and_forms() {
    let h = harness("en");
    for label in ["Organize pages", "Combine files", "Compare files", "Edit a PDF", "Protect a PDF"] {
        assert_eq!(cards(&h, label), 1, "{label} card");
    }
    for label in ["Add comments", "Prepare a form"] {
        assert_eq!(cards(&h, label), 0, "{label} card");
    }
    h.get_by_label("Open file");
}

#[test]
fn the_compare_card_opens_compare_files_with_overlay() {
    let mut h = harness("en");
    let card = h.query_all_by_label("Compare files").find(|n| (n.rect().width() - 190.0).abs() < 1.0).expect("Compare files card");
    card.click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("compare"));
    h.get_by_label("Overlay two revisions");
}

#[test]
fn the_combine_card_opens_the_combine_tab() {
    let mut h = harness("en");
    click_card(&mut h, "Combine files");
    assert_empty_combine(&h);
}

/// COMBINE.1: the card asks for no file picker. A picker answered by `pick_override` would add its
/// file to the list.
#[test]
fn the_combine_card_opens_no_file_picker() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-combine1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pdf = dir.join("picked.pdf");
    std::fs::write(&pdf, one_page()).unwrap();
    let path = pdf.to_string_lossy().into_owned();
    let mut h = harness_with("en", move |app| app.pick_override = Some(vec![path]));
    click_card(&mut h, "Combine files");
    h.run_steps(6);
    assert_empty_combine(&h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pinned_combine_card_opens_the_combine_tab_too() {
    let mut h = harness_with("en", |app| {
        assert!(app.pin_tool("combine"));
    });
    assert_eq!(cards(&h, "Combine files"), 1, "pinned once, not recommended again");
    click_card(&mut h, "Combine files");
    assert_empty_combine(&h);
}

/// An open Combine tab keeps its files: the card shows that list again, it doesn't start over.
#[test]
fn the_combine_card_keeps_an_open_combine_list() {
    let mut h = harness_with("en", |app| {
        app.use_files(FilePurpose::Combine, vec![("one.pdf".into(), one_page()), ("two.pdf".into(), one_page())]);
    });
    assert!(h.state().combine_showing());
    h.get_by_label("Home").click();
    h.run_steps(3);
    assert!(!h.state().combine_showing() && h.state().combine_tab.open);
    click_card(&mut h, "Combine files");
    let app = h.state();
    assert!(app.combine_showing());
    let names: Vec<&str> = app.combine_draft.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["one.pdf", "two.pdf"]);
}

/// Only the Home card changed: the All tools row still opens the Combine tool pane, and the other
/// cards still open theirs.
#[test]
fn other_ways_to_combine_and_other_cards_are_unchanged() {
    let mut h = harness("en");
    h.state_mut().left = LeftPanel::AllTools;
    h.run_steps(3);
    h.query_all_by_label("Combine files")
        .find(|n| (n.rect().width() - 190.0).abs() >= 1.0 && n.rect().top() > 40.0)
        .expect("the All tools row")
        .click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("combine"));
    assert!(!h.state().combine_showing());

    let mut h = harness("en");
    click_card(&mut h, "Organize pages");
    assert_eq!(h.state().left, LeftPanel::Tool("organize"));
    assert!(!h.state().combine_showing());
}

#[test]
fn the_cards_are_translated_in_spanish() {
    if crate::support::skip_without_catalogs("the_cards_are_translated_in_spanish") {
        return;
    }
    let h = harness("es");
    for label in ["Combinar archivos", "Comparar archivos"] {
        assert_eq!(cards(&h, label), 1, "{label} card");
    }
}
