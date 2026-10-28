//! This fork (HOME.1): Home recommends Organize pages, Combine files, Compare files (with Overlay),
//! Edit a PDF and Protect a PDF. Each card opens its existing tool panel; comments and forms stay
//! in All tools.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{LeftPanel, PdfCraftApp};

fn harness(language: &'static str) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", language).unwrap();
        app
    });
    h.run_steps(4);
    h
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
fn the_combine_card_opens_combine_files() {
    let mut h = harness("en");
    let card = h.query_all_by_label("Combine files").find(|n| (n.rect().width() - 190.0).abs() < 1.0).expect("Combine files card");
    card.click();
    h.run_steps(3);
    assert_eq!(h.state().left, LeftPanel::Tool("combine"));
}

#[test]
fn the_cards_are_translated_in_spanish() {
    let h = harness("es");
    for label in ["Combinar archivos", "Comparar archivos"] {
        assert_eq!(cards(&h, label), 1, "{label} card");
    }
}
